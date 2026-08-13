# Example: Relay Resolution

```markdown
## Relay resolution — Task #42

**Decision:** Keep DTR records owned by their historical placement.

### Reason

Placement-level ownership preserves historical accuracy and matches the
approved requirement that previous placement records remain intact.

Total credited hours should be calculated across the placement chain.

### Implementation direction

- Do not move existing DTR records.
- Do not change DTR ownership semantics.
- Aggregate credited hours across qualifying placements.
- Preserve historical placement reporting.

### Scope

This resolution only addresses DTR ownership during reassignment.
Other placement behavior remains unchanged.

### Work disposition

Keep existing partial work and continue.
```
