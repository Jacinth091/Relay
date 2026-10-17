# PR #12 Governed Remediation Waves

**Source:** external governed review of PR #12 (`feat/v2-architecture-rebaseline`, fixed reviewed head `8fc1566d`). The review route was **RETURN TO DEV**: substantial v2 scaffolding exists, but it is not wired into the production execution path end-to-end, most v2 orchestration state is memory-only, and phase-completion documentation materially overstated what was implemented.

The full audit (governed PR review, phase-by-phase implementation audit, and one detailed correction plan per finding) is maintained outside this repository as the review's evidentiary record. This document records the wave structure and finding titles in-tree so implementation work has an authoritative local reference instead of depending on an out-of-band artifact.

The review's own remediation policy: **do not repair all 19 findings inside another undifferentiated mega-commit.** Apply the waves in order; each correction is a bounded, self-contained unit of work; later waves depend on earlier ones being accepted first.

## Wave A — Restore architectural truth and one owner

Status: **LANDED, re-review pending**

- **R12-18** — Phase completion and documentation are materially inconsistent with the implementation
- **R12-01** — Canonical daemon ownership is not wired into production clients
- **R12-03** — Durable command/effect idempotency is crash-unsafe
- **R12-04** — Most v2 orchestration state is not durable

### Wave A acceptance checklist

- [x] `ROADMAP.md` phase statuses reset to evidence-based values (no phase marked `DONE` without a proven production exit gate).
- [x] `docs/v2_architecture_rebaseline/README.md` corrected: points to accepted ADR-0020, no longer instructs readers to create a conflicting ADR-0019.
- [x] TUI and MCP task-submission paths route through a real out-of-process `DaemonClient` (named pipe transport), not direct `SqliteStore` writes. (`AppState::confirm_task_import`, `mcp::service::execute_submit_handoff`; the still-v1 Run/scheduler path is an explicit, documented exception pending Wave B/R12-02.)
- [x] `relay daemon` actually listens for and serves requests (not just a lease heartbeat loop). (`relay_core::run_full_daemon` + `daemon_transport::run_named_pipe_listener`.)
- [x] Daemon commands are crash-safe: same command id + same payload is idempotent; same id + different payload fails closed; a crash between apply and completion is reconciled on restart without duplicating the underlying effect. (`DaemonServer::resolve_command`/`reconcile_on_startup`, deterministic task ids via `TaskId::deterministic_from_command_id`, `needs_reconciliation` status for undecodable payloads; covered by `crates/relay-core/tests/wave_a_command_idempotency.rs` and the real-pipe hard-kill test in `apps/relay-tui/tests/daemon_cli.rs`.)
- [x] `PendingEffect` records are executed/reconciled by a daemon-owned worker instead of sitting `pending` indefinitely. (`relay_core::effect_worker::run_effect_worker`, spawned by `run_full_daemon`.)
- [x] Docs re-synced (pass 2) to describe what actually landed, without re-claiming full phase completion.

### Known residual gaps carried into Wave B

- The v1 `ExecutionScheduler` Run path still writes to `SqliteStore` directly; cutting it over to the v2 orchestrator is R12-02.
- The daemon lease-reclaim window after an unclean crash (not a graceful shutdown) is still bounded by the full lease TTL, matching pre-existing lease semantics; not addressed by this wave.
- No effect handlers are registered yet (the worker's registry is proven empty-but-functional); Wave B/C register real handlers as corrected paths start producing effects.

## Wave B — Establish a real v2 execution vertical slice

Status: **PARTIAL — R12-05, R12-06, R12-07 and R12-08 landed; R12-02 outstanding**

- **R12-05** — Project trust classification can become trusted without verifying the expected remote — **landed** (`17866e6`)
- **R12-06** — Workflow, intent, and policy contracts are below the rebaseline specification — **landed** (`9f96216`)
- **R12-07** — Candidate/artifact/intent integrity digests are not stable cryptographic digests — **landed** (`b142aca`)
- **R12-08** — Lease fencing and expiry validation are incorrect — **landed** (`db97bb4`); the durable-checkout lease expiry path was corrected and regression-tested by PR #16 promotion-review finding **R16-01** (see the corrections below)
- **R12-02** — The user-facing Run path still executes the v1 scheduler, not the v2 orchestrator — **outstanding**

### PR #16 promotion-review corrections

The governed review of PR #16 (`staging` @ `0080ba8`) returned **FAIL** with four
P1 and two P2 findings. The corrections landed on `fix/v2-pr16-promotion-corrections`
(no R12-02 work was mixed in):

- **R16-01** — durable-checkout lease expiry never fired for the epoch-seconds text
  the runtime writes, so an expired holder could not be reclaimed. Epoch seconds is
  now canonical end to end (integer SQL comparison, no date parsing of epoch text)
  and an unparseable expiry fails closed instead of counting as live. Regression
  coverage: `durable_lease_expiry_is_reclaimed_once_ttl_elapses`,
  `durable_lease_with_live_ttl_still_blocks_competing_holder`,
  `durable_resource_lease_reclaims_expired_epoch_seconds_lease`. R12-08's "landed"
  language above is only accurate because of this correction.
- **R16-02** — the daemon transport was a process-global pipe name, so a client
  could submit into another database's daemon. The pipe is now derived from the
  database's durable state identity and every request is refused on an identity
  mismatch (`DaemonResponse::StateIdentityMismatch` → typed
  `DaemonError::StateIdentityMismatch`). Regression coverage:
  `a_pipe_client_cannot_reach_a_daemon_serving_a_different_database`,
  `two_databases_never_cross_write_on_the_default_pipe_identity`,
  `serve_mcp_writes_only_to_its_own_database_when_another_daemon_is_running`,
  `handshake_refuses_a_client_bound_to_a_different_database`.
- **R16-03** — the daemon resolved submissions against the registry it captured at
  boot, so a project added while it ran was rejected. Submissions now re-read the
  persisted trusted registry (`Relay::config_path`). Regression coverage:
  `project_registered_after_the_daemon_started_is_immediately_usable`,
  `submission_uses_a_project_registered_after_daemon_start`,
  `a_handoff_for_a_project_registered_after_daemon_start_is_accepted`,
  `out_of_process_daemon_honours_a_project_registered_after_start`.
- **R16-04** — a failed heartbeat used to stop only the heartbeat loop, leaving a
  stale daemon serving commands. Losing the lease now marks authority lost (every
  request fails closed), stops the listener and effect worker, and exits the
  `relay daemon` process. Regression coverage:
  `losing_the_lease_stops_the_daemon_serving_and_releases_its_pipe`,
  `a_daemon_that_lost_its_lease_refuses_every_request`.
- **R16-05** — concurrent replays of one command id could both apply it and record
  conflicting terminal status. A non-terminal command is now claimed by exactly one
  caller (migration `0008`, `commands.claimed_at`); the loser waits for the winner's
  committed result. Regression coverage:
  `concurrent_replays_of_one_command_id_converge_on_one_result`.
- **R16-06** — the PR #16 description was resynced to the exact reviewed head and
  the owner's accepted deferrals; no status table was rewritten.

### PR #16 re-review corrections (R17)

The governed re-review of the corrected head (`048f5c9`) returned **BLOCK /
RETURN TO DEV**: R16-01, R16-03 and R16-06 were **CLOSED**, while R16-02, R16-04
and R16-05 were **PARTIAL**, with two P2 follow-ups. The R17 corrections land on
`fix/v2-pr16-rereview-corrections` (again with no R12-02 work mixed in). The
review's own finding text was verified against source before any change.

- **R17-01 (re-opens R16-02)** — the typed client verified identity, but the
  *server* did not: a raw pipe peer could send `SubmitCommand` with no handshake
  at all, or after a failed one, and it would be dispatched. A connection is now
  admitted only after a matching-identity handshake succeeds
  (`daemon_transport::serve_connection`); every other request on an unadmitted
  connection is refused without being dispatched. Each mutating request also
  carries the `state_identity` it is bound to, so the direct request path
  (`DaemonServer::handle_request`) fails closed too. The daemon wire protocol
  version is raised to 2 for that field. Regression coverage:
  `raw_peer_without_a_handshake_cannot_submit_a_command`,
  `mismatched_handshake_does_not_admit_the_connection`,
  `matching_handshake_admits_subsequent_commands`,
  `submit_command_requires_the_matching_database_identity`.
- **R17-02 (re-opens R16-04, first half)** — `run_full_daemon` spawned the
  listener and returned immediately, so a successor could report a running daemon
  whose pipe had never bound (a bind failure was only logged). Startup now waits
  for the listener to actually bind, retries binding within a bounded window while
  a stale instance still holds the pipe, and fails startup truthfully if it cannot
  bind. Regression coverage: `a_daemon_waits_for_a_held_pipe_and_then_serves`,
  `a_daemon_that_cannot_bind_its_pipe_reports_startup_failure`.
- **R17-03 (re-opens R16-04, second half)** — in-flight fencing was missing: a
  command admitted while this process was authoritative could still commit after a
  successor took the lease. Every daemon command write is now fenced inside its own
  transaction on the lease's fencing token (`DaemonFence`,
  `assert_daemon_fence`); a stale write is refused with
  `StoreError::DaemonAuthorityLost`, and a command refused that way is left
  non-terminal (not falsely recorded as `failed`) for the successor to reconcile.
  Regression coverage:
  `fenced_writes_are_refused_after_a_successor_takes_the_daemon_lease`.
- **R17-04 (re-opens R16-05)** — `resolve_command` discarded `complete_command`'s
  result and still reported success, a crash between the compare-and-set commit
  and the terminal write recorded a false `failed` on replay, and a cached
  `failed` command was returned as `Duplicate`. Completion errors are now
  propagated (never an optimistic `applied`); a replay whose transition is already
  present converges on `applied`; and a cached `failed` command replays as
  `failed`. Regression coverage:
  `replaying_a_committed_state_transition_reports_applied_not_failed`,
  `a_failed_command_replays_as_failed_not_duplicate`,
  `concurrent_replays_of_one_command_id_converge_on_one_result`.
- **R17-05 (P2)** — the durable-lease expiry comment claimed the store "tolerated"
  date-text rows; the store actually compares the column as an integer, so such a
  row reads as expired. The comment now states the real invariant.
- **P2 follow-up, not fixed here** — the two-database migration race (identical
  `SQLITE_BUSY`/duplicate-column classification gap at startup) remains a separate
  tracked item: it is a migration-time defect that R17's command-path fencing does
  not reach, and the reviewer recommended tracking it separately rather than
  widening this correction.

### PR #16 second re-review corrections (R18)

The governed re-review of the R17 head (`0ff8807`) confirmed the Windows CI
evidence (37 test groups, 443 passed, 0 failed, 0 ignored) but returned
**BLOCK / RETURN TO DEV**: R16-02, R17-01 and R17-05 **CLOSED**; R16-04, R16-05,
R17-02, R17-03 and R17-04 **PARTIAL**, because two authority-boundary holes
remained. The R18 corrections land on `fix/v2-pr16-r18-corrections` (no R12-02
work mixed in). Both findings were reproduced in source before any change.

- **R18-01 (re-opens R16-04/R16-05/R17-03/R17-04, command ledger)** — R17-03
  fenced the *semantic* writes but left the **command ledger** unfenced:
  `record_command`, `claim_command`, `complete_command` and `reset_command_claims`
  were plain writes with no authority check. A command admitted while this process
  was authoritative could therefore, *after* a successor had taken the lease and
  already run its one-time `reset_command_claims` + reconciliation, still
  `claim_command` itself durably and then fail its semantic fence — leaving a
  non-terminal command claimed with nobody left to clear it until the next
  restart. Separately, `complete_command` was an unconditional
  `UPDATE ... WHERE id = ?3`, so a delayed stale completion could overwrite a
  newer terminal result (e.g. turn a successor's `applied` into `failed`).
  The store now exposes fenced variants — `record_command_fenced`,
  `claim_command_fenced`, `complete_command_fenced`,
  `reset_command_claims_fenced` — each asserting `assert_daemon_fence` as the
  first statement of its own transaction, and `complete_command_fenced` is
  additionally guarded on the command still being non-terminal
  (`status IN ('received','needs_reconciliation')`): it returns
  `Ok(false)` when another applier already recorded a terminal result (that
  result stands, and the caller reports the committed outcome) and a typed
  `StoreError::CommandNotFound` for a genuinely absent row. The daemon command
  path uses only the fenced variants. The unfenced variants remain for
  store-level tests that seed or rewind the ledger, and are documented as
  authority-unchecked; in particular the unfenced `complete_command` keeps its
  unconditional rewrite because the crash-window tests depend on being able to
  rewind a terminal row to a non-terminal one. Regression coverage:
  `command_ledger_writes_are_refused_after_a_successor_takes_the_daemon_lease`,
  `a_terminal_command_is_not_overwritten_by_a_duplicate_completion`,
  `a_daemon_that_lost_the_lease_cannot_write_the_command_ledger` (stale daemon
  cannot write the ledger, no row is created, the successor resolves the same
  command id to exactly one truthful terminal state, a late stale completion is
  refused, and no command is left stuck).
- **R18-02 (re-opens R16-04/R17-02, startup truthfulness)** — two defects in the
  bind-retry path. First, if the shared shutdown fired while the listener was
  still retrying a pipe held by a predecessor, the listener sent its **success**
  signal (`ready.send(())`) and returned, so `run_full_daemon` reported a running
  daemon whose transport had never bound. The readiness channel now carries a
  truthful outcome (`Result<(), String>`): bound, cancelled, or an error, and the
  cancellation branch reports the cancellation instead of readiness. Second, the
  failure arms tore down only the listener — the heartbeat task was left running
  and the lease acquired by the failed startup was never released, pinning the
  daemon lease until its TTL elapsed. Every post-acquisition failure path now
  broadcasts the shared shutdown, joins **both** the heartbeat and the listener,
  and then releases the lease; `release_daemon_lease` is scoped to
  `lease_id + holder_pid`, so a failed startup can release only the lease it still
  owns and can never delete a successor's row. Regression coverage:
  `cancelled_startup_does_not_report_a_bound_listener`,
  `a_startup_cancelled_while_binding_fails_and_keeps_the_successor_lease`,
  `a_daemon_that_cannot_bind_its_pipe_reports_startup_failure` (now also asserts
  the failed startup did not pin the lease).
- **WARN-02 (optional hardening)** — a stale instance could still acknowledge a
  matching raw-pipe handshake after its authority had been marked lost, which
  reads as "healthy" to a caller treating the ack as proof of authority.
  `handshake_response` now fails closed when authority is lost, so the connection
  is never admitted.
- **WARN-01 / DOC-02** — the tracked migration race remains an owner-accepted
  exclusion (unchanged); the PR #16 description was resynced to the corrected head
  after this branch landed.

### PR #16 third re-review corrections (R19)

The governed re-review of the R18 head (`d980cfe`) confirmed the R18 corrections
(R18-01 and R18-02 **CLOSED**) but returned **BLOCK / RETURN TO DEV**: three
further authority/lifecycle paths contradicted the single-writer and
truthful-startup contract. R19 lands on `fix/v2-pr16-r19-authority` off
`staging` (fix commit `ddafeb7`), with no R12-02 / Wave B work mixed in. All
three were reproduced in source before any change.

- **FLOW-19-01 (effect-worker writes — an adjacent, reachable ledger)** — the
  daemon's effect worker mutated the same canonical `pending_effects` /
  `effect_results` ledger the daemon owns, but through unfenced, unconditional
  writes (`update_effect_state`, `record_effect_result`). A worker suspended
  across a successor takeover could therefore flip a `completed` effect back to
  `failed` and leave a contradictory second result row beside the successor's.
  The worker now carries the daemon's `DaemonFence` and writes through
  `update_effect_state_fenced(.., expected_state, fence)` (claim or requeue only
  from the state it observed; `Ok(false)` when the claim moved on, typed
  `PendingEffectNotFound` when the row is absent) and
  `settle_effect_from_worker_fenced(..)` (result plus settle in one transaction
  guarded by the fence and `state = 'in_flight'`), so a late result can neither
  regress a settled effect nor add a second outcome. Losing the lease stops the
  worker instead of letting it keep polling without authority. Regression
  coverage: `effect_worker_writes_are_fenced_and_cannot_regress_a_settled_effect`
  (asserts the raw `effect_results` rows) and
  `a_stale_effect_worker_result_is_refused_after_a_successor_takes_the_lease` (a gated
  handler holds the worker inside dispatch while the successor settles the
  effect, which makes that interleaving deterministic rather than timing-based).
- **DB-19-02 (lease identity was process-scoped)** — daemon leases were owned by
  `(holder_pid, holder_host)`. Two daemon instances inside one process share both,
  so the second instance renewed the first's fencing token instead of being
  refused, ran its own `reconcile_on_startup` over the live instance's ledger, and
  its failed-startup cleanup deleted the lease row the live first instance was
  using. Migration **0009** adds `daemon_leases.holder_instance`. Renewal matches
  pid + host + instance; a different instance is refused (`DaemonLeaseHeld`) while
  the row is fresh and is a normal stale takeover (token + 1) once the TTL has
  elapsed; `release_daemon_lease` is scoped to the owning instance, which
  supersedes the pid-only scoping described for R18 above; and `DaemonFence` /
  `assert_daemon_fence` compare the instance too, because a released row can be
  re-acquired at the same pid with the token restarting at 1, so pid plus token
  cannot name a generation. `DaemonServer::fence()` becomes `pub` so the effect
  worker and the external integration tests can present the same authority; a
  fence stays a read-only value whose every use is still checked against the live
  lease row. Regression coverage:
  `daemon_lease_ownership_is_per_instance_not_per_process`,
  `a_reacquired_lease_does_not_accept_the_previous_instances_fence`, and
  `a_second_daemon_in_one_process_cannot_share_or_delete_the_first_lease` (a
  second `run_full_daemon` in one process is refused, the live instance's command
  claim is not reset, its lease and token survive, and it still serves commands).
- **FLOW-19-03 (readiness after a cancelled startup)** — the listener published
  readiness as soon as the pipe was created, with no shutdown check, so a
  cancellation after `create` succeeded was reported as a running daemon whose
  listener exited on its next select. The listener now checks the shared shutdown
  immediately before publishing readiness (no await between the check and the
  send) and reports the cancellation instead; `run_full_daemon` re-checks the same
  state before returning a successful handle, through the new
  `readiness_failure`. Regression coverage:
  `a_shutdown_set_before_readiness_is_not_reported_as_a_bound_listener` and
  `a_readiness_signal_under_a_cancelled_startup_is_not_readiness`. The remaining
  interleaving (a shutdown landing after the listener's check but before the host
  returns) is closed **by construction** rather than by a test hook: no
  `run_full_daemon`-level barrier test was added, because that would put a
  test-only parameter on a production listener. This arm is disclosed to the
  reviewer to grade rather than claimed as covered by a failing-before test.
- **Bounded extras, each disclosed** — migration 0009 (required by DB-19-02); one
  added pattern in `is_migration_race` for SQLite's "duplicate column name"
  message, required so a concurrent first migrator of 0009 is classified like the
  already-accepted WARN-01 class (the pattern also covers 0002 and 0008, which
  `ALTER` the same way); and a documentation note on
  `RecoveryCoordinator::reconcile_startup`, which writes effects unfenced but has
  no daemon caller (its only caller is the phase-15 test), so any future
  daemon-side caller must fence it exactly as the worker now does.
- **Carried forward, deliberately deferred** — the optional P2 from this review (a
  dedicated "handshake against a daemon whose authority was lost" test; the
  behaviour itself landed in R18 WARN-02) is **not** in R19 scope and stays an
  open P2 for the next round. WARN-01 (multi-process first-migration race) remains
  the owner-accepted exclusion; 0009 joins that class.
- **Migration numbering** — 0009 is now taken by this round, so Wave B's parked
  migrations must start at **0010**.

## Wave C — Harden runtime/provider semantics

Status: **NOT STARTED**

- **R12-09** — LocalProcessBackplane does not yet provide the required containment/idempotency semantics
- **R12-11** — Provider process exit success is incorrectly treated as semantic result publication
- **R12-10** — Herdr integration is a mock vertical shape, not a durable production backplane

## Wave D — Make orchestration durable/recoverable

Status: **NOT STARTED**

- **R12-12** — Relay Exchange is memory-only and materialized outbox ingestion is not idempotent
- **R12-13** — Delegation and Scheduler v2 are nonrecoverable in-memory algorithms
- **R12-14** — Policy enforcement classes overstate what Relay actually enforces

## Wave E — Privilege and failure correctness

Status: **NOT STARTED**

- **R12-15** — Privileged approvals are not bound to the trusted checkout and one-shot state is not durable
- **R12-16** — Startup recovery converts ambiguous external effects into false failures
- **R12-17** — Retention can delete fresh or still-referenced worktrees

## Wave F — User surfaces and final acceptance

Status: **NOT STARTED**

- **R12-19** — There is no exact-current-head quality-gate evidence

## Phase 12 Web UI rule

Do **not** implement the Web UI merely to make `ROADMAP.md` look complete while the daemon/orchestrator is still incorrect. Waves A-E must be accepted first; the Web UI is then implemented in its own bounded PR against the original Phase 12 contract.

## Re-review fixed point

After all waves are remediated, pin a new exact head, rerun the governed review stack, verify every previously blocking finding against code and tests, and require exact-head quality gates before forwarding to staging.

**Deferred by owner decision (2026-10-16).** The v2 branch was forwarded to `staging` (PR #12) with Wave A landed and Wave B partially landed, ahead of this fixed point, so the private repository could be prepared for experimental hobby/development use. The governed re-review and Waves C-F remain outstanding; they are the gate on claiming v2 complete and on publishing the repository, not merely on promoting `staging` to `main`. Nothing in this promotion resolves a previously blocking finding on its own, and no finding may be marked resolved except by the exact-head re-review defined above.
