# Example: Relay Needs a Decision

```markdown
## Relay needs a decision — Task #42

**Project:** PractiCore
**State:** NEEDS_DECISION
**Runtime:** Codex
**Reason:** Architecture decision required

### What I was implementing

The approved task requires placement reassignment while preserving
historical practicum records and previously credited DTR hours.

### What has been implemented so far

- Added the reassignment service flow.
- Preserved the previous placement instead of overwriting it.
- Added validation for the replacement placement.
- Existing tests for normal placement creation still pass.

### What caused the stop

While implementing historical DTR preservation, the current data model
does not clearly define whether DTR records should remain owned by the
historical placement or move to a practicum-level record.

### Relevant context discovered

Current relationship:

    DTR -> Placement

Changing ownership to:

    DTR -> Practicum Record

would affect existing queries and reporting.

Keeping the current relationship would require aggregation across the
placement chain instead.

### Decision needed

Should historical DTR records remain attached to the placement where
they were originally recorded?

### Options identified

**A — Keep DTR ownership on each historical placement**
- preserves accurate placement history;
- requires aggregation across placements;
- smaller schema change.

**B — Move DTR ownership to a practicum-level record**
- simplifies total-hours queries;
- changes existing ownership semantics;
- requires broader schema/service changes.

### Executor assessment

Option A appears more consistent with the approved requirement to
preserve historical placement records.

This is a recommendation only.

### Current repository state

Modified:
- src/services/placement.service.ts
- src/services/dtr.service.ts

Validation:
- typecheck: PASS
- existing placement tests: PASS
- reassignment tests: BLOCKED

### Resume point

Once the ownership decision is provided, implementation can continue
without re-planning unrelated work.
```
