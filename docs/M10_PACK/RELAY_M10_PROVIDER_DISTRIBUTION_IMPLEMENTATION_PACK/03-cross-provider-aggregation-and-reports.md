# M10 Phase 03 — Cross-Provider Aggregation and Reports

## Goal

A human reviewing a fanned-out handoff sees one consolidated view of all sibling outcomes, a deterministic conflict summary between provider outputs, and — on material disagreement — a structured `NEEDS_DECISION` packet. The Dashboard DISTRIBUTION section groups sibling rows under their parent handoff.

## Current state

Exists today:

- Reports are per-execution: `ExecutionReport` carries final state, outcome kind, bounded output snapshot (`crates/relay-core/src/execution.rs`); execution records persist per task in SQLite.
- The Dashboard DISTRIBUTION section counts running/completed work per profile from durable records and the scheduler snapshot (`distribution_rows` in `apps/relay-tui/src/util.rs`, rendered by the dashboard builder). It aggregates nothing beyond presentation (ADR-0018, item 5).
- Escalation packets already exist for single-execution flows and surface in the ESCALATION INBOX.

Does not exist:

- Any cross-sibling comparison, consolidated verdict view, or disagreement-driven escalation.

## Requirements

1. **Consolidated result view.** For a parent task with siblings, the execution/result screen shows one section listing every sibling with provider label, profile, final state, outcome summary, and its retained output. Sibling outputs remain individually inspectable; consolidation never hides a leg.
2. **Deterministic conflict/diff summary only.** The comparison between sibling outputs is mechanical: identical-vs-divergent outcome kinds, exit summaries, verification results, and bounded-output text equality. Divergence is reported as "siblings disagree" with the concrete differences excerpted from stored records. No LLM summarization, no similarity scoring, no natural-language verdicts (invariant 7). If two siblings' outputs differ only in trivially-stripped whitespace they may be classed as agreeing; any other difference is divergence, erring toward escalation.
3. **Escalation on material disagreement.** When sibling outcomes are mixed DONE/FAILED, or DONE siblings' verification results diverge, Relay emits one `NEEDS_DECISION` packet for the parent containing: the approved intent reference, per-sibling outcome table, the deterministic diff summary, and explicit candidate choices ("accept codex output / accept antigravity output / discard all"). Resolution validates through the existing resolution-packet path before anything resumes (`docs/clarification-protocol.md`). Executor recommendations inside packets are not approvals.
4. **DISTRIBUTION evolution.**
   - Parent-grouped rows: a fanned-out handoff renders as one parent line with indented per-sibling lines (provider, state), replacing today's flat per-profile counting for those tasks.
   - Single-execution tasks render exactly as today; the flat count remains the fallback for non-fan-out work so existing tests and behavior stay stable.
   - Presentation computes nothing about *which* sibling wins; it displays rollup state and packet pointers only.
5. **Store/report additions.** A read API to fetch siblings-by-parent plus their records, in `relay-store`; aggregation is a pure function over stored data so it is testable without runtimes.
6. **Tests:** aggregation purity unit tests over fixture records; escalation-packet content for each disagreement shape; UI tests for grouped rows (see phase 04).

## Boundaries

- Aggregation reads local durable state only; no network calls, no runtime queries at render time.
- No automatic acceptance of any sibling, ever (invariant 14).
- Packet format changes stay within the clarification protocol's validation rules.

## Done when

- A fake-runtime fan-out with deliberately divergent outcomes produces a NEEDS_DECISION packet whose diff summary matches the recorded outputs exactly.
- Dashboard shows parent-grouped sibling rows alongside unchanged single-execution rows.
- Full workspace gates pass.
