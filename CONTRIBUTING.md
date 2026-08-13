# Contributing to Relay

Relay is private during initial development.

These rules establish the intended contribution style before public release.

## Before Coding

Read:

1. `README.md`
2. `contexts/PROJECT_CONTEXT.md`
3. `docs/design-principles.md`
4. `docs/architecture.md`
5. relevant ADRs

## Change Ownership

Determine the correct layer before implementation:

- core;
- ingress;
- runtime;
- stack;
- planner specification / provider adapter;
- presentation client.

Do not put provider-specific shortcuts in core.

## Architectural Changes

If a change affects a product invariant, task protocol, security boundary, persistence strategy, runtime contract, or presentation independence:

- add/update an ADR;
- update relevant docs;
- add boundary tests.

## Expected Checks

When Rust implementation exists:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

## Pull Requests

Prefer small, coherent changes.

Describe:

- problem;
- layer changed;
- security implications;
- tests;
- documentation impact.

## Public Contribution Policy

A public code of conduct, contribution workflow, and license policy will be added before open-source release.
