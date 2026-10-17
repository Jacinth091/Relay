# ADR-0020: Relay Architecture v2 — Policy-Governed Workflow Orchestration Control Plane

## Status

Accepted for Relay Architecture v2 (2026-09-17).  
Supersedes [ADR-0017](0017-relay-identity-deterministic-handoff-engine.md) and the incompatible unimplemented portions of [ADR-0018](0018-provider-distribution-model.md).  
Historical M0–M9 lineage and accepted milestone snapshots remain frozen as historical evidence.

## Context

Relay's original architectural generation (v0.1 / M0–M9) established a robust, deterministic handoff engine:
- Local trusted project resolution and Git repository safety (M2–M3);
- Canonical `HandoffTask v1` protocol and ingress validation (M4);
- Durable SQLite persistence and single-writer concurrency (M5, M9);
- Single scheduler with slot bounds, recovery, and bounded repair loops (M6);
- TUI review with mandatory human Run approval (M7);
- Codex runtime integration with JSONL app-server transport and process isolation (M8);
- Relay Direct/MCP submission server via `relay serve-mcp` (M9).

Under ADR-0017, Relay was defined strictly as a deterministic handoff engine: execution orchestration only, never intent orchestration. Under ADR-0018, provider distribution was conceived as fan-out/slot distribution across runtimes without role separation, workflows, execution grants, or candidate management.

However, real-world multi-agent software engineering requires:
1. Multi-role collaboration (e.g., Planner, Implementer, Verifier, Reviewer, Remediator);
2. Candidate management and generation-bound verification with repository-state fingerprinting;
3. Bounded delegation and consultations without uncontrolled recursive AI-to-AI spawning;
4. Integration with interactive terminal workspace managers (Herdr) as execution backplanes while preserving a minimal local-process backplane;
5. Optional advisory semantic decision intelligence (such as TypeSafe AI Jev) without compromising deterministic correctness or human intent authority.

Continuing to stretch the v1 single-turn handoff engine framing creates architectural ambiguity. An explicit generational break to **Relay Architecture v2** is required.

## Decision

### 1. Product Identity and Canonical Ownership

Relay v2 is a **local-first, policy-governed orchestration control plane for user-approved software work**.

Canonical authority model:
- **Human**: Owns intent, approves objectives and constraints, grants execution, resolves scope-expanding decisions, and approves privileged actions.
- **Relay (Daemon)**: Canonical owner of durable tasks, approved intent, execution grants, workflow definitions, role instances, role attempts, candidates, leases, artifacts, exchange, policies, and recovery.
- **Runtime Backplane**: Hosts process/session resources and reports observations. Does not own tasks, decide policy, or determine completion.
- **Provider Adapter**: Translates provider-specific launch, readiness, and result formats (Codex, Claude, AGY, OpenCode).
- **Provider CLI**: Performs one assigned role attempt (`RoleAttempt`).
- **Semantic Decision Engine**: Optional advisory engine (e.g., Jev) for bounded recommendations after deterministic filtering; never an authority.

Summary anchor:
> *"The human owns intent. Relay owns controlled orchestration. Runtime backplanes host execution. Providers perform assigned roles."*

### 2. Historical Freeze and Supersession

- **Freeze v1 Lineage**: The accepted M0–M9 milestone lineage is preserved as immutable historical implementation evidence. Prior ADRs (0001–0016, 0019) remain valid historical foundations.
- **ADR-0017 Superseded**: The restriction that Relay cannot coordinate multi-step roles, workflows, or bounded delegation is superseded. Relay v2 coordinates policy-governed workflows while maintaining deterministic authority over lifecycle, persistence, permissions, and safety.
- **ADR-0018 / M10 Pack On Hold**: The unbuilt provider fan-out and M10 provider-distribution pack are superseded in favor of v2 role-based workflow execution targets and provider resolution.
- **Documentation Area**: A dedicated `docs/v2_architecture_rebaseline/` area tracks the forward v2 architecture and sequential implementation plan.

### 3. Core Domain Primitives

Relay v2 introduces clear domain separation:
```text
Provider != Role != RoleAttempt != Skill != Policy != RuntimeProfile
```

Key primitives:
- `ProjectId` & `DeclaredProjectIdentity`: Logical repository identity, distinct from filesystem paths.
- `TrustedProjectRegistration` & `CheckoutId`: Local trusted root, remote fingerprint, and allowed worktree boundaries.
- `Task`: User-visible work objective in durable aggregate state (`READY`, `RUNNING`, `SUSPENDED`, `NEEDS_CLARIFICATION`, `NEEDS_DECISION`, `NEEDS_APPROVAL`, `COMPLETED`, `FAILED`, `CANCELLED`).
- `ApprovedIntent`: Immutable approved objective, acceptance criteria, constraints, and non-goals. Revised scope creates a new generation via `ScopeExpansionRequest`.
- `ExecutionGrant`: Explicit authorization created upon human Run, defining allowed roles, budget, policy snapshot, and checkout scope.
- `WorkflowDefinition` & `WorkflowRun`: Declarative, validated graph of typed inputs, roles, transitions, and bounds.
- `RoleDefinition` & `RoleInstance`: Semantic role responsibility (Implementer, Verifier, etc.) and activation.
- `RoleAttempt`: Concrete provider and runtime execution attempt with an immutable `ExecutionTarget`.
- `Candidate`: Generation-bound repository state frozen with full fingerprint (HEAD, index, diff checksum, untracked manifest).
- `Lease`: Monotonically fenced ownership token preventing conflicting writes.
- `Artifact` & `Exchange`: Provenance-tracked, checksummed, typed work products exchanged across role boundaries.

### 4. Runtime Backplane Seam

Relay v2 establishes a clean, decoupled `RuntimeBackplane` interface (`provision`, `observe`, `send`, `interrupt`, `dispose`, `snapshot`, `reconcile`):
- `FakeBackplane`: Deterministic testing infrastructure.
- `LocalProcessBackplane`: Direct process spawning with argument arrays and trusted CWD (minimal non-Herdr production path).
- `HerdrBackplane`: Default/reference interactive backplane. Herdr internal topology (workspace/tab/pane) is managed privately inside the adapter; Relay core remains independent of Herdr topology concepts.

### 5. Preserved Invariants and Compatibility Obligations

Relay v2 explicitly commits to:
1. **`HandoffTask v1` Compatibility**: Existing JSON tasks submitted via MCP, paste, or local file are validated and adapted internally into `ApprovedIntent` (generation 1) and a compatibility `WorkflowRun` upon Run.
2. **Submission Semantics**: `submit_handoff` via `relay serve-mcp` continues to mean *validate -> resolve trusted project -> persist -> READY*. It never triggers automatic execution.
3. **Explicit Run Approval**: No workflow executes without an explicit human Run action creating an `ExecutionGrant`.
4. **No Automatic Git Push**: Relay never pushes to remote repositories automatically.
5. **Trusted Local Path Resolution**: Tasks cannot inject arbitrary filesystem paths; working directories derive solely from trusted local project registrations.
6. **Provider Quotas and Authentication**: Provider authentication, billing, and subscription controls remain owned by provider CLIs; Relay never bypasses them.
7. **Fail-Closed Safety**: Unknown protocols, malformed outputs, and security mismatches fail closed.

### 6. Implementation Strategy

v2 is implemented in 16 dependency-complete vertical slices (Phases 00 through 15) per `02-IMPLEMENTATION-SEQUENCE.md`. The initial end-to-end proof uses deterministic fake infrastructure before Herdr or Jev is required.

## Consequences

- Relay gains a coherent multi-role, workflow-driven architecture without compromising safety.
- The daemon becomes the single canonical writer of durable state, effects, and events.
- Tools and clients (Web, TUI, MCP) interact with the daemon over authenticated local protocols.
- Core domain logic remains completely decoupled from specific runtime backplanes (Herdr) and external semantic engines (Jev).
