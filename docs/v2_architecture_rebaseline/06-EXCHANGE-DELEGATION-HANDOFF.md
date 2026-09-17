# Relay v2 Exchange and Delegation Handoff

# 1. Objective

Create a typed, durable, scoped communication layer so AI CLIs cooperate through Relay rather than direct provider-to-provider spawning or terminal transcript scraping.

Relay Exchange is part of the orchestration authority because it controls which information/results a role may access/publish.

# 2. Communication Rule

Normal pattern:

```text
RoleAttempt
-> scoped inbox / artifacts
-> provider performs role
-> publishes typed result/message
-> Relay validates and persists
-> workflow/dependent role consumes accepted artifact
```

Do not use direct AI-to-AI terminal conversation as the canonical protocol.

# 3. Channels

## Messages

Small coordination objects:

- question;
- consultation request/response;
- progress signal;
- bounded correction request;
- delegation status.

## Artifacts

Durable typed outputs:

- Specification / Intent reference;
- ImplementationPlan;
- AnalysisResult;
- ImplementationResult;
- CandidateReference;
- VerificationResult;
- ReviewResult;
- RemediationResult;
- IntegrationPlan;
- OperationPlan/Result;
- Patch/Diff;
- RawLog.

# 4. Artifact Trust Classes

Every artifact states one of:

```text
ProviderClaim
RelayObserved
Derived
```

Examples:

- provider says `changed_files = [...]` -> `ProviderClaim`;
- Relay computes Git diff/fingerprint -> `RelayObserved`;
- Relay derives a summary from several accepted records -> `Derived`.

Policies/workflows can require Relay-observed evidence for load-bearing decisions.

# 5. Scoped Inbox

A RoleAttempt sees only:

- artifacts declared by its role/workflow inputs;
- messages addressed to it;
- explicitly mounted project context;
- allowed parent/child consultation data.

It cannot enumerate all Relay projects/tasks by default.

# 6. Cross-Task / Cross-Project Isolation

Default:

```text
same RoleAttempt -> same Task/WorkflowRun only
```

Cross-task reference in one project requires an explicit grant/mount.

Cross-project reference requires a stronger explicit grant and policy support.

Do not infer access merely because files are readable by the same OS user.

# 7. Interface vs OS Isolation

Exchange can enforce access through its own API/CLI/MCP surface.

If the provider process has unrestricted same-user filesystem access, it may technically bypass Exchange and read local Relay files. Therefore:

- store sensitive files with restrictive permissions where possible;
- prefer sandbox-enforced filesystem scopes for stronger guarantees;
- label Exchange isolation as mediated unless OS sandbox is active;
- never claim API authorization alone is a complete filesystem sandbox.

# 8. Role-Session Capability

Issue one opaque capability per RoleAttempt.

Bind to:

```text
ProjectId
CheckoutId
TaskId
WorkflowRunId
RoleInstanceId
RoleAttemptId
allowed operations
expiry
```

Properties:

- high entropy/opaque;
- short-lived;
- revoked at attempt end/cancel;
- not embedded in artifacts;
- not copied into repository files;
- not written to normal prompts/logs;
- caller IDs cannot broaden claims.

# 9. CLI / Tool Surfaces

Possible role-scoped CLI:

```text
relay exchange inbox
relay artifact get <artifact-id>
relay result publish <path>
relay delegate <role> --inputs ...
relay delegation status <id>
relay consult <role> --question ...
```

Planner submission surface remains separate:

```text
relay serve-mcp
  -> submit_handoff
  -> Ready task only
```

Role-scoped MCP/tool endpoints require role-session capability and expose only role operations.

# 10. Materialized Session Fallback

For providers that cannot reliably call Relay tools:

```text
~/.relay/runtime/<role-attempt-id>/
├── SESSION.json
├── INBOX.json
├── input/
└── outbox/
```

Rules:

- ephemeral projection only;
- canonical artifacts remain in artifact store;
- restrictive local permissions where available;
- no unrelated project/task data;
- no long-lived capability secrets in files unless an adapter-specific secure mechanism is deliberately designed;
- cleanup after attempt according to retention/recovery policy.

# 11. Result Publication

```text
provider publishes result
-> Relay parses schema
-> validates task/workflow/role/attempt scope
-> validates candidate generation/fingerprint reference where required
-> validates required fields
-> stores immutable artifact
-> marks result accepted
-> RoleInstance may advance
```

Malformed/incomplete result:

```text
RESULT_INVALID
-> optional one bounded correction request
-> still invalid -> attempt/role BLOCKED or FAILED according to workflow
```

Do not infer load-bearing fields from conversational prose.

# 12. Runtime Exit vs Result Publication

Possible states:

```text
result accepted + process still running
process exited + result accepted
process exited + no result
process lost + result already accepted
```

Workflow uses accepted result semantics, not a simple process-exit condition.

# 13. Delegation Request

Providers request semantic work, not executables/providers directly.

Example:

```json
{
  "requested_role": "verifier",
  "mode": "BLOCKING_CHILD",
  "purpose": "verify the current candidate against acceptance criteria",
  "input_artifacts": ["candidate-7", "intent-1"]
}
```

Relay chooses legal provider/runtime target.

# 14. Delegation Validation

Before creating a child:

1. authenticate parent RoleAttempt;
2. validate input artifacts are visible;
3. validate requested role is allowed by ExecutionGrant;
4. validate scope remains inside ApprovedIntent;
5. evaluate policy;
6. check execution budget;
7. check max depth/count;
8. check blocking dependency graph for cycle/deadlock;
9. validate child role capabilities/independence;
10. persist delegation + child RoleInstance before scheduling.

# 15. Delegation Modes

## Blocking child

Parent waits for child result. Dependency graph must be acyclic.

## Nonblocking child

Child result may feed a later workflow step but parent can complete independently if workflow permits.

## Consultation

Bounded question/answer relationship. No implicit write authority.

# 16. Agent-Initiated Verification

Allowed only if ExecutionGrant/workflow permits Verifier delegation.

Flow:

```text
Implementer publishes Candidate N
-> releases/ends write lease as required
-> asks for Verifier
-> Relay acquires verification lease/fencing token
-> applies independence policy
-> Verifier attempt
-> VerificationResult tied to Candidate N
```

If candidate changes, result invalidates.

# 17. Agent-Initiated Review

Reviewer inputs include exact candidate and required context.

Independence may require:

- fresh session;
- producer context excluded;
- different provider preferred/required.

Reviewer cannot convert a non-approved scope expansion into ordinary findings and then authorize it itself.

# 18. Agent-Initiated Planning / Replanning

A Planner/Debugger may propose a new plan inside ApprovedIntent.

If proposed work materially expands scope, publish `ScopeExpansionRequest` and stop at human decision boundary.

# 19. Provider Replacement

Provider replacement creates a new RoleAttempt under the same RoleInstance.

```text
RoleInstance Verifier V1
  Attempt 1 Codex -> quota exhausted
  Attempt 2 Claude -> result accepted
```

Parent/workflow consumes the accepted role result without pretending two different semantic verifiers existed unless that is actually desired.

# 20. Attempt Context Independence

For fresh independent attempts, Relay controls which artifacts/context are mounted.

`ProducerContextExcluded` means do not automatically provide producer conversation/transcript/internal scratch context. Required specification/candidate evidence still remains.

# 21. Execution Budget Accounting

Each successful child creation/attempt updates durable budget usage.

Check limits transactionally before activation where possible.

Budget exhaustion produces a deterministic workflow event/attention state, not an uncontrolled spawn failure.

# 22. Raw Logs

Raw logs may be artifacts but:

- are bounded;
- can be sensitive;
- are not automatically sent to every downstream role;
- are fetched only when needed;
- do not replace typed result schemas.

# 23. Conversation Parsing

Conversation-log parsing is a diagnostic/provider recovery fallback only because it is:

- provider-specific;
- prone to truncation;
- noisy;
- hard to scope to candidate generations;
- likely to contain secrets/chatter;
- poor for deterministic recovery.

If used, record adapter/parser version and provenance as a provider claim/derived artifact.

# 24. Artifact Storage

Large payload flow:

```text
write temporary file
-> fsync/close as appropriate
-> compute checksum
-> atomic rename into artifact store
-> persist metadata
```

On mismatch/corruption, mark artifact unusable and fail/recover explicitly.

# 25. Artifact Retention

Retention classes may include:

```text
EPHEMERAL_SESSION
TASK_LIFETIME
PROJECT_HISTORY
AUDIT_REQUIRED
```

No cleanup while referenced by active workflow/effect/approval or required audit retention.

# 26. Acceptance Criteria

1. normal role cooperation uses typed Exchange.
2. role capability cannot widen its task/project scope.
3. cross-task/project access is denied by default.
4. filesystem-isolation limitations are documented truthfully.
5. malformed result cannot complete a role.
6. provider replacement uses new RoleAttempt, same RoleInstance.
7. delegation stays within ApprovedIntent/ExecutionGrant.
8. blocking dependency cycles are rejected.
9. budget prevents unbounded role/attempt growth.
10. Candidate verification remains generation-bound.
11. direct provider spawning is not an orchestration path.
12. raw logs remain secondary evidence.
