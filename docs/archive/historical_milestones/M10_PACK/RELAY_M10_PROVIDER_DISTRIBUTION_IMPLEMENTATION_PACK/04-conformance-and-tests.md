# M10 Phase 04 — Conformance Tests and Documentation Reconciliation

## Goal

Prove capability dispatch, fan-out lifecycle, cancellation propagation, and disagreement escalation against the deterministic fake runtime; pin the new Dashboard behavior with UI tests; reconcile docs so shipped distribution behavior and remaining design intent are clearly separated.

## Conformance scenarios (fake runtime, in-process unless noted)

All scenarios use `relay-runtime-fake` scriptable outcomes; no real provider CLI is required for this suite.

1. **Router capability dispatch.**
   - Two registered providers declaring overlapping profiles resolve by metadata, not prefix;
   - a profile resolvable only through metadata (provider id not derivable from the profile string) routes correctly;
   - an unsatisfiable policy fails closed with the typed error before any runtime start is attempted.
2. **Health-aware selection determinism.**
   - With a preference-set intent, identical health snapshots always select the same provider across repeated runs;
   - degrading the preferred provider's snapshot yields either that provider (if intent named it explicitly — error path) or the configured next candidate, never an arbitrary one;
   - absent snapshots behave as today (conservative defaults), proving probes are optional inputs.
3. **Slot accounting under fan-out.**
   - N=3 fan-out under default limits: exactly `max_parallel` siblings run, remainder queue with existing queue-status semantics;
   - per-project limit holds across siblings of the same parent exactly as it does across distinct tasks today;
   - sibling completion frees its slot for a queued sibling deterministically.
4. **Cancel-parent-cancels-siblings.**
   - Parent cancellation while two siblings run and one is queued: all three report cancelled individually, parent rolls up to Cancelled, zero live fake-runtime sessions remain;
   - single-sibling cancellation leaves the other siblings running and the rollup rule from phase 02 requirement 5 applies at settle time.
5. **Disagreement escalation.**
   - Scripted outcomes covering every branch: all DONE -> DONE; any escalation -> parent escalates with sibling-named packet; DONE/FAILED mix -> NEEDS_DECISION packet with per-sibling table and diff summary matching stored records byte-for-byte; all FAILED -> FAILED;
   - resolution-packet validation rejects tampered packets before resume (existing clarification-protocol rules).
6. **Protocol compatibility regression.** A fan-out run's stored task payload is byte-identical to a non-fan-out run's payload; ingress still rejects non-empty extensions end-to-end.

## UI test expectations (`apps/relay-tui/src/tests.rs`)

- Dashboard DISTRIBUTION renders parent-grouped indented sibling rows for fanned-out tasks and unchanged flat rows for single-execution tasks (extend `dashboard_distribution_counts_running_executions`).
- Run approval surface exposes the fan-out provider selection and refuses an empty or unknown-provider set.
- Escalation inbox shows one NEEDS_DECISION entry per disagreeing parent, not one per sibling.
- Consolidated result screen lists each sibling with provider label and state; no sibling is ever hidden.

## Documentation reconciliation

All edits carry honest exists-today labels; nothing ships as "working" until its conformance scenarios pass repeatedly on Windows:

- `docs/status-2026-08.md`: M10 outcome — which ADR-0018 design-intent items are now implemented vs still open.
- `docs/roadmap.md`: tick fan-out / health-aware routing / aggregation as they land.
- `docs/adr/0018-provider-distribution-model.md`: add a short implementation-status note pointing at this pack (the ADR text itself stays authoritative for decisions).
- `docs/runtime-stack-model.md` and `docs/architecture.md`: replace name-prefix routing descriptions with capability-metadata dispatch once phase 01 merges; note the stack-allowlist migration state honestly if partial.
- `docs/tui-ux.md`: document the grouped DISTRIBUTION rows and consolidated result view.
- `AGENTS.md` status snapshot: move provider distribution from "design intent only" to its actual post-M10 state.

## Done when

- Every scenario above passes reliably (run several times locally).
- Full workspace gates pass (`cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features`, `cargo test --workspace`).
- Docs updated with accurate status labels.
- A human has completed at least one real two-provider fan-out locally, including one deliberate disagreement escalated to NEEDS_DECISION and resolved through the TUI.
