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
