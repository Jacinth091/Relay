# ADR-0019: Multi-Runtime Orchestration Architecture

> **Numbering note:** this ADR was originally filed as 0009-multi-runtime-orchestration.md, colliding with 0009-single-relay-ingress-model.md. It was renumbered to 0019 without any change to its decision content.

**Status:** Proposed
**Date:** 2026-08-15
**Scope:** M8.1 â€” Multi-Runtime Orchestration + Terminal UX Overhaul

## Context

M8 proved that Relay can discover, authenticate, launch, stream, and receive structured outcomes from the Codex CLI runtime. However, Relay currently supports only a single hardcoded runtime provider (Codex), and the TUI behaves more like a debug console than a mature orchestration surface.

The user cannot:
- See which runtime/model is being used at a glance
- Switch between runtime providers
- Inspect runtime capabilities before execution
- View normalized execution activity separate from raw provider logs
- Inspect repository diffs or execution logs in dedicated views

## Decision

### Runtime Management Layer

Add a provider-neutral runtime management layer to `relay-runtime`:

1. **`RuntimeProfile`** â€” A trusted local execution policy describing provider, model, reasoning effort, filesystem capability, network policy, duration, and repair budget.
2. **`RuntimeIntegration`** trait â€” Separate from the execution `Runtime` trait. Provides discovery, health probing, profile listing, model catalog, profile resolution, and capability verification.
3. **`RuntimeRouter`** â€” Routes execution requests to the appropriate provider runtime based on profile ID.
4. **`RuntimeActivity`** â€” Provider-neutral normalized execution events (CommandStarted, FileChanged, VerificationCompleted, etc.) for TUI consumption.
5. **`CapabilityStatus`** â€” Distinguishes Configured vs Verified vs Unknown vs Mismatch for honest capability reporting.

### Provider Adapters

- **Codex adapter** (`relay-runtime-codex`): Implements `RuntimeIntegration` in addition to `Runtime`.
- **Antigravity adapter** (`relay-runtime-antigravity`): New crate implementing both `Runtime` and `RuntimeIntegration` for Google Antigravity CLI.
- **Fake runtime** (`relay-runtime-fake`): Development/testing only, unchanged.

### Terminal UX

The TUI transitions from a debug dashboard to a command-first orchestration console:

- **Runtime Dashboard** (`/runtimes`): Overview of all registered providers with health, profiles, and models.
- **Runtime Detail** (`/runtime <provider>`): Detailed view of a specific provider.
- **Runtime Switching** (`/runtime use <profile>`): Easy runtime/profile switching.
- **Diff Viewer** (`/diff`): Read-only repository diff from trusted local Git state.
- **Logs Viewer** (`/logs`): Raw execution output.
- **Enhanced Doctor** (`/doctor`): Multi-provider health diagnostics.

### Selection Precedence

```
explicit one-run selection
    â†“
project default runtime profile
    â†“
Relay global default runtime profile
    â†“
fail closed if no safe profile resolves
```

## Preserved Boundaries

This change preserves all accepted contracts:

- M2: Trusted project paths remain from local config
- M3: Repository safety remains mandatory before execution
- M5: SQLite remains canonical durable state
- M6: Single scheduler, no second scheduler per provider
- M7: Explicit Run approval remains mandatory
- M8: Provider adapters execute already-approved runtime requests

## Consequences

- Relay becomes runtime-neutral at the orchestration level
- Adding a new provider requires only a new adapter crate
- Users can switch between providers without restart
- Provider-specific settings stay in adapters, not in core
- Runtime profiles are trusted local state (tasks cannot escalate permissions)


