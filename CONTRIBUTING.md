# Contributing to Relay

Relay is an **experimental personal hobby/development project**, made public so people can learn, try ideas, and propose improvements. Issues and pull requests are welcome, but there is no guarantee of review, acceptance, support, or response time. Be constructive and respectful.

## Start here

Read `README.md`, `docs/DEVELOPMENT.md`, `AGENTS.md`, `contexts/PROJECT_CONTEXT.md`, `docs/design-principles.md`, `docs/architecture.md`, and the relevant ADRs. Check existing issues before starting substantial work.

For larger changes, discuss the problem in an issue first. Keep PRs focused and avoid expanding the architecture to support purely hypothetical providers.

## Architecture and boundaries

Determine the owning layer: core, protocol, store, runtime, planner integration, or presentation. Core is provider-agnostic. Preserve explicit human approval, safe path/project resolution, durable state invariants, and fail-closed security behavior. Changes to contracts, product invariants, or persistence require corresponding documentation/ADR and boundary tests.

## Validation

Run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked
cargo test --workspace --locked
```

Clearly state which checks were run, skipped, or failed. Do not claim untested OS or provider compatibility.

## Pull requests

Include the problem, scope, affected architectural layers, risk/security implications, tests, and documentation updates. Be willing to split large PRs. Do not submit secrets, private logs, personal information, production credentials, private task handoffs, or unlicensed third-party content.

## Licensing

Unless otherwise noted, contributions submitted for inclusion in the repository are offered under its [MIT License](LICENSE). By contributing, you represent that you have the rights to submit the material and agree to its distribution under that license. No CLA is currently required.

Security issues should be reported privately, following [SECURITY.md](SECURITY.md), rather than by opening public issues.
