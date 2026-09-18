# Relay — Runtime Model & Reasoning Control Implementation Plan

**Status:** DEV implementation plan  
**Date:** 2026-08-15  
**Scope:** Model selection, reasoning/effort selection, runtime-profile overrides, per-project defaults, per-run tuning, execution snapshotting, Codex provider wiring, and TUI controls  
**Current source branch:** `feat/m8-codex-runtime`  
**Primary concern:** Runtime profiles currently behave as rigid presets instead of tunable orchestration policies.

---

# 1. Problem Statement

Relay already models:

```text
runtime provider
profile
model
reasoning/effort
filesystem capability
network capability
duration
repair budget
```

but the current implementation does not give the orchestrator meaningful control over the AI-specific parameters.

Current Codex behavior is effectively:

```text
codex/review
  model = default
  reasoning = medium

codex/implement
  model = default
  reasoning = high

codex/deep
  model = default
  reasoning = xhigh
```

The profile ID therefore implicitly decides the model/reasoning behavior.

That is too rigid.

Relay should instead treat a Runtime Profile as:

> **a trusted safety/execution baseline with adjustable provider-native AI parameters.**

Model and reasoning are not the same kind of setting as:

```text
workspace access
network
extra writable roots
commit
push
```

The latter are safety boundaries.

Model and reasoning/effort are **orchestration tuning controls** and should be adjustable by the local user.

---

# 2. Current Source Mismatch

## RuntimeProfile already supports the fields

Current:

```rust
pub struct RuntimeProfile {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub model: String,
    pub reasoning_effort: ReasoningEffort,
    ...
}
```

So the provider-neutral runtime model already has the right concepts.

## Runtime settings already anticipate overrides

Current:

```rust
pub struct ProfileOverrideConfig {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub max_duration_secs: Option<u64>,
}
```

This is the correct configuration direction.

## Codex command execution does not use the resolved values

Current Codex command builder selects:

```text
sandbox
network
cwd
schema/result
```

but model comes from:

```rust
self.config.model_override
```

rather than the resolved runtime profile.

Reasoning effort is not translated into Codex invocation configuration at all.

Therefore:

```text
RuntimeProfile.model
RuntimeProfile.reasoning_effort
ProfileOverrideConfig.model
ProfileOverrideConfig.reasoning_effort
```

are currently mostly descriptive configuration rather than execution authority.

---

# 3. Product Decision

Freeze this rule:

> **Runtime profiles provide safe defaults. Model and reasoning/effort remain locally adjustable.**

Example:

```text
Codex / Implement

Safety baseline
  Workspace write      On
  Network              Off
  Extra write roots    None
  Auto commit          Off
  Auto push            Off

AI defaults
  Model                Default
  Reasoning            High
```

The user may change:

```text
Model
Reasoning
Duration within limits
```

without creating a new safety profile.

The user may **not** casually change:

```text
workspace access
network
extra roots
commit
push
```

through the same lightweight tuning control.

Those remain governed by trusted profiles and future explicit safety policy.

---

# 4. Required Selection Layers

Runtime configuration should resolve in layers.

```text
Built-in profile defaults
        ↓
trusted saved profile override
        ↓
trusted project override
        ↓
one-run local override
        ↓
validated EffectiveRuntimeSelection
        ↓
frozen into execution
```

Suggested precedence:

```text
1. explicit one-run model/reasoning override
2. project runtime tuning override
3. saved profile override
4. built-in profile default
5. provider/account default where Relay intentionally leaves a field unset
```

---

# 5. Distinguish Safety Profile From Runtime Tuning

Keep:

```text
codex/review
codex/implement
codex/deep
```

but change their meaning.

## `codex/review`

Defines:

```text
filesystem        read-only
network           off
repairs           low
duration          bounded

default model     provider/account default
default reasoning medium
```

## `codex/implement`

Defines:

```text
filesystem        workspace-write
network           off
repairs           normal
duration          bounded

default model     provider/account default
default reasoning high
```

## `codex/deep`

Defines:

```text
filesystem        workspace-write
network           off
repairs           higher but bounded
duration          longer

default model     provider/account default
default reasoning xhigh
```

The profile still matters.

But it no longer prevents the user from saying:

```text
Codex / Implement
Model       GPT-5.3-Codex
Reasoning   Medium
```

or:

```text
Codex / Implement
Model       account default
Reasoning   XHigh
```

when the provider/model supports it.

---

# 6. New Provider-Neutral Runtime Selection Snapshot

Do not pass only:

```text
profile_id
```

to the provider.

Add a provider-neutral resolved snapshot in `relay-runtime`.

Recommended:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeExecutionSelection {
    pub profile_id: String,

    /// None means use provider/account default.
    pub model: Option<String>,

    /// None means use provider/profile default.
    pub reasoning_effort: Option<ReasoningEffort>,
}
```

Alternative:

```rust
pub struct RuntimeExecutionSelection {
    pub profile_id: String,
    pub model: RuntimeModelSelection,
    pub reasoning_effort: RuntimeReasoningSelection,
}
```

with explicit enums:

```rust
enum RuntimeModelSelection {
    ProviderDefault,
    Explicit(String),
}

enum RuntimeReasoningSelection {
    ProfileDefault,
    Explicit(ReasoningEffort),
}
```

The explicit enum form is preferred because:

```text
"default"
```

should not be a magic provider model string.

---

# 7. Extend ExecutionRunPolicy

Current:

```rust
ExecutionRunPolicy {
    runtime_profile_id,
    max_duration,
    max_repair_attempts,
}
```

Change to:

```rust
ExecutionRunPolicy {
    runtime: RuntimeExecutionSelection,
    max_duration,
    max_repair_attempts,
}
```

or:

```rust
ExecutionRunPolicy {
    runtime_profile_id,
    model,
    reasoning_effort,
    max_duration,
    max_repair_attempts,
}
```

Preferred:

```text
one nested RuntimeExecutionSelection
```

because model/reasoning belong to runtime selection, not generic scheduler policy.

---

# 8. Freeze Runtime Tuning At Queue Time

Once the user presses Run:

```text
profile
model
reasoning effort
duration
repair budget
```

must become immutable for that execution.

Example:

```text
Task A queued:

Provider    Codex
Profile     Implement
Model       GPT-5.3-Codex
Reasoning   High
```

Then the user changes global defaults to:

```text
Model       account default
Reasoning   Medium
```

Task A remains:

```text
GPT-5.3-Codex / High
```

Only later runs use the new defaults.

---

# 9. Extend RuntimeRequest

Current:

```rust
RuntimeRequest {
    ...
    profile_id: String,
}
```

Replace or augment with:

```rust
RuntimeRequest {
    ...
    runtime_selection: RuntimeExecutionSelection,
}
```

The provider adapter receives the **fully resolved local selection**.

It must not re-read mutable global defaults after the execution has started.

---

# 10. Effective Profile Resolution

`RuntimeIntegration::resolve_profile()` should still resolve the safety baseline.

Add a second merge step:

```text
built-in RuntimeProfile
        +
ProfileOverrideConfig
        +
project override
        +
one-run override
        ↓
EffectiveRuntimeSelection
```

Possible API:

```rust
fn resolve_execution_selection(
    &self,
    profile_id: &str,
    overrides: &RuntimeTuningOverrides,
) -> Result<ResolvedRuntimeSelection, RuntimeConfigError>;
```

Provider adapter validates provider-specific support.

---

# 11. RuntimeTuningOverrides

Introduce:

```rust
#[derive(Debug, Clone, Default)]
pub struct RuntimeTuningOverrides {
    pub model: Option<RuntimeModelSelection>,
    pub reasoning_effort: Option<ReasoningEffort>,
}
```

This is trusted local state.

Never source it from:

```text
HandoffTask arbitrary fields
extensions
provider stdout
remote GitHub issue
```

unless a later explicitly approved policy permits a requested non-authoritative hint.

---

# 12. Saved Global/Profile Overrides

Use existing:

```text
runtimes.toml
```

Example:

```toml
[runtime]
default_profile = "codex/implement"

[profile_overrides."codex/implement"]
model = "gpt-5.3-codex"
reasoning_effort = "high"
max_duration_secs = 1800
```

Or:

```toml
[profile_overrides."codex/implement"]
model = "default"
reasoning_effort = "medium"
```

Interpret:

```text
model = "default"
```

as:

```text
ProviderDefault
```

at the config boundary.

Do not pass the literal word `"default"` to Codex as a model ID.

---

# 13. Project-Level Runtime Tuning

Extend the trusted runtime settings schema without touching M2 Project Registry.

Example:

```toml
[projects.relay]
profile = "codex/implement"
model = "gpt-5.3-codex"
reasoning_effort = "high"

[projects.practicore]
profile = "codex/implement"
model = "default"
reasoning_effort = "medium"
```

If keeping the existing:

```rust
projects: HashMap<String, String>
```

for backward compatibility, introduce a versioned v2 migration such as:

```rust
ProjectRuntimeSettings {
    profile: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
}
```

Load v1 safely.

Do not modify the strict M2 project registry format.

---

# 14. One-Run Override

Task Review must allow temporary tuning.

Example:

```text
Task

Project       relay
State         READY

Runtime
  Codex / Implement

AI
  Model       GPT-5.3-Codex
  Reasoning   High

> Run
  Change model
  Change reasoning
  Change runtime/profile
  Save as project default
  Back
```

Choosing:

```text
Change reasoning
```

opens:

```text
Reasoning

  Provider/Profile default
  Low
  Medium
> High
  XHigh
```

Only values supported by the selected provider/model should be enabled where support is known.

---

# 15. Model Selector UX

Codex runtime detail:

```text
Runtime / Codex / Implement

Model
  Account/CLI default

Reasoning
  High

> Model
  Reasoning
  Save profile defaults
  Set as project default
```

Selecting Model:

```text
Model

> Use provider default
  GPT-5.3-Codex
  GPT-5.6
  Custom model ID...
```

Important:

Do **not** maintain a stale hard-coded model catalog.

Preferred:

```text
provider-supported discovery
```

If Codex CLI does not expose a stable machine-readable catalog:

```text
Use provider default
Custom model ID...
```

is safer than inventing a static list.

Optionally populate known provider models only from a separately refreshed/provider-supported source.

---

# 16. Reasoning Selector UX

Reasoning is separate from Profile.

Example:

```text
Reasoning effort

  Profile default (High)
  Low
  Medium
> High
  XHigh
```

Do not expose unsupported levels as valid.

When model capability is known:

```text
GPT-5.3-Codex
  Low
  Medium
  High
  XHigh
```

When capability is unknown:

```text
show provider-supported generic values
validate at provider preflight
```

Fail with an actionable message rather than silently downgrading.

---

# 17. Codex Provider Wiring — Model

Codex CLI currently supports a per-invocation model override through:

```text
--model / -m
```

Therefore:

```text
RuntimeModelSelection::ProviderDefault
→ omit --model

RuntimeModelSelection::Explicit(model)
→ --model <model>
```

Do not use the adapter-global:

```rust
CodexRuntimeConfig.model_override
```

as the primary runtime selection mechanism.

That field may remain temporarily for backward compatibility but should be merged into trusted settings during composition.

---

# 18. Codex Provider Wiring — Reasoning

Codex currently supports per-invocation configuration overrides using:

```text
-c key=value
```

and its configuration includes:

```text
model_reasoning_effort
```

Therefore the Codex adapter should translate:

```text
ReasoningEffort::Low
→ -c model_reasoning_effort="low"

ReasoningEffort::Medium
→ -c model_reasoning_effort="medium"

ReasoningEffort::High
→ -c model_reasoning_effort="high"

ReasoningEffort::XHigh
→ -c model_reasoning_effort="xhigh"
```

`Default` / unset:

```text
omit explicit reasoning override
```

The exact TOML argument encoding must be regression-tested against the current installed Codex CLI.

---

# 19. Do Not Couple Reasoning to Filesystem Profile

Current profile names imply:

```text
review → medium
implement → high
deep → xhigh
```

Those remain defaults.

Do not implement logic such as:

```rust
if profile == "implement" {
    reasoning = High;
}
```

inside the command builder.

The command builder must consume the already-resolved execution selection.

---

# 20. Codex Command Builder Target

Conceptual:

```rust
fn command_spec(
    &self,
    request: &RuntimeRequest,
    effective_profile: &EffectiveRuntimeProfile,
    ...
)
```

Build:

```text
codex
--ask-for-approval never
exec
--ignore-user-config
...
--sandbox <resolved filesystem safety profile>

[--model <selected model>]

[-c model_reasoning_effort="<selected effort>"]

...
```

The provider adapter translates the resolved profile.

It does not invent the selection.

---

# 21. `--ignore-user-config` Makes Relay Tuning More Important

Relay intentionally uses:

```text
--ignore-user-config
```

to prevent the user's Codex configuration from widening the execution envelope.

That is good for safety.

But it also means Relay cannot rely on the user's ordinary Codex model/reasoning configuration for the execution.

Therefore Relay must intentionally pass the desired:

```text
model
reasoning effort
```

when the user selected them.

Otherwise the orchestration layer removes provider customization without replacing it.

---

# 22. Runtime Dashboard

`/runtime codex` should become tunable.

Target:

```text
Runtime / Codex

Status
  ✓ Installed
  ✓ Authenticated
  ✓ Compatible

Default profile
  Codex / Implement

AI defaults
  Model        GPT-5.3-Codex
  Reasoning    High

Safety
  Workspace    Write
  Network      Off
  Extra roots  None

> Change profile
  Change model
  Change reasoning
  Test runtime
  Save settings
```

This is where runtime orchestration becomes genuinely useful.

---

# 23. `/runtime use` vs AI Tuning

Keep:

```text
/runtime use codex/implement
```

for choosing the **profile**.

Add either command-based tuning:

```text
/runtime model codex gpt-5.3-codex
/runtime reasoning codex high
```

or preferably contextual runtime-detail selection.

Avoid making the command language excessively large.

Recommended primary UX:

```text
/runtime codex
→ interactive model/reasoning selector
```

with optional quick commands later.

---

# 24. Command Palette Context

When inside:

```text
/runtime codex
```

or Task Review, completion may expose actions:

```text
Change model
Change reasoning
Save as default
Use once
```

Do not force users to remember model IDs.

---

# 25. Provider-Specific Terminology

Shared Relay concept:

```text
AI effort
```

Provider presentation:

```text
Codex
  Reasoning

Antigravity
  Effort
```

The runtime-neutral enum can remain `ReasoningEffort` if practical.

The TUI should use provider-native language where it improves clarity.

---

# 26. Antigravity

Apply the same orchestration principle.

Its profile may default:

```text
model    default
effort   high
```

but the user can tune those independently from:

```text
filesystem
sandbox
provider mode
```

Provider adapter translates them through the provider-native supported interface.

Do not make Codex and Antigravity command flags identical.

---

# 27. Validation Layer

Before Run:

```text
provider exists
profile resolves
model selection accepted/known
reasoning effort compatible where known
runtime healthy
repository eligible
capability envelope valid
```

Then freeze the selection.

If model/effort cannot be validated before execution:

```text
show Effective/Compatibility = Unknown
```

and let the provider return an explicit configuration error.

Do not silently substitute another model or reasoning level.

---

# 28. Execution View

Display the exact frozen selection:

```text
Codex / Implement
GPT-5.3-Codex · High · Write · Offline
```

This must come from the execution snapshot.

Not:

```text
current global default
```

If user changes settings during execution, the running view remains unchanged.

---

# 29. Result View

Persist and show:

```text
Runtime      Codex
Profile      Implement
Model        GPT-5.3-Codex
Reasoning    High
```

This is important for:

```text
reproducibility
review
debugging
dogfood comparison
cost/performance comparison
```

---

# 30. Execution Persistence

The current execution record already persists `runtime_profile`.

Extend the additive runtime metadata snapshot to include:

```json
{
  "provider": "codex",
  "profile": "codex/implement",
  "model": "gpt-5.3-codex",
  "reasoning_effort": "high",
  "runtime_version": "...",
  "filesystem": "workspace_write",
  "network": false
}
```

Do not store credentials.

---

# 31. Repair Attempts

Default behavior:

```text
Repair attempt uses the same:
  provider
  profile
  model
  reasoning effort
```

Do not silently escalate:

```text
Medium → XHigh
small model → large model
```

during repair.

A future explicit adaptive-execution policy can decide that.

For now execution configuration remains stable.

---

# 32. Configuration Migration

Current:

```rust
ProfileOverrideConfig {
    model,
    reasoning_effort,
    max_duration_secs,
}
```

Keep this shape.

The required work is mostly:

```text
load it
parse it
merge it
validate it
pass it
display it
persist it
```

not inventing a new unrelated configuration subsystem.

---

# 33. Reasoning Parser

Add strict:

```rust
fn parse_reasoning_effort(value: &str) -> Result<ReasoningEffort, ...>
```

Support only the Relay/provider-supported values.

Examples:

```text
default
low
medium
high
xhigh
```

Do not silently map typos.

---

# 34. Model Parsing

Treat model ID as provider-owned opaque text after basic safety validation.

Reject:

```text
empty
control characters
newlines
```

Do not:

```text
interpret as shell
interpret as path
allow task input to supply it
```

Provider adapter passes it as a single process argument.

---

# 35. Tests — Runtime Settings

Required:

```text
built-in profile only
saved model override
saved reasoning override
project override
one-run override
precedence
invalid reasoning
empty model
backward-compatible v1 settings
```

---

# 36. Tests — Codex Command Builder

Required:

```text
ProviderDefault model
→ no --model

Explicit model
→ --model exact-id

Default reasoning
→ no explicit reasoning override

High
→ model_reasoning_effort=high

XHigh
→ model_reasoning_effort=xhigh

task text containing --model
→ remains stdin content

task text containing model_reasoning_effort
→ remains stdin content
```

---

# 37. Tests — Frozen Queue Selection

Scenario:

```text
global:
  codex/implement
  model A
  reasoning High

queue Task A

change global:
  model B
  reasoning Medium

queue Task B
```

Expected:

```text
Task A → model A / High
Task B → model B / Medium
```

---

# 38. Tests — Repair Stability

Task begins:

```text
GPT-5.3-Codex / High
```

Verification fails.

Repair request must remain:

```text
GPT-5.3-Codex / High
```

unless a future adaptive policy explicitly changes it.

---

# 39. Tests — UI

Task Review must show:

```text
model
reasoning
profile
```

Changing model/reasoning:

```text
changes only local pending Run selection
does not mutate task
does not queue
```

Pressing Run:

```text
freezes selection
```

---

# 40. Manual Codex Acceptance

Use a disposable Relay task.

1. Configure:

```text
Codex / Implement
Model      provider default
Reasoning  Medium
```

Run.

2. Configure a second execution:

```text
Codex / Implement
Model      selected supported model
Reasoning  High
```

Run.

Verify from execution metadata/provider invocation evidence that the selections differ.

Do not rely only on what the TUI label says.

---

# 41. UX Acceptance

The user must be able to:

```text
/runtime codex
    ↓
see current model/reasoning
    ↓
change model
    ↓
change reasoning
    ↓
save as profile/global default
```

and:

```text
Task Review
    ↓
change model/reasoning for this Run only
    ↓
Run
```

without creating new profiles manually.

---

# 42. Hard Completion Gate

Do **not** call runtime tuning complete if:

```text
[ ] profiles still permanently dictate reasoning
[ ] RuntimeProfile.model is not passed to execution
[ ] RuntimeProfile.reasoning_effort is not passed to execution
[ ] Codex still reads only CodexRuntimeConfig.model_override
[ ] reasoning effort never reaches Codex
[ ] user cannot change model from Relay
[ ] user cannot change reasoning from Relay
[ ] per-run changes mutate global settings
[ ] global setting changes affect queued/running tasks
[ ] result does not record model/reasoning
[ ] runtime UI says one configuration while provider receives another
```

---

# 43. Recommended Implementation Order

```text
1. Add RuntimeExecutionSelection
2. Extend ExecutionRunPolicy
3. Extend RuntimeRequest
4. Merge profile + runtime settings overrides
5. Freeze selection at Run
6. Wire Codex model
7. Wire Codex reasoning
8. Persist runtime metadata
9. Add Runtime Detail tuning controls
10. Add Task Review one-run tuning
11. Add project-level tuning
12. Add tests
13. Dogfood two different Codex configurations
```

---

# 44. Recommended Commits

```text
1. feat[Jacinth]: carry runtime model and effort through execution

2. fix[Jacinth]: make Codex execution use resolved runtime tuning

3. feat[Jacinth]: add runtime model and reasoning controls

4. test[Jacinth]: cover frozen runtime tuning and Codex overrides
```

---

# 45. Quality Gates

```bash
cargo metadata --no-deps --format-version 1
cargo tree --workspace
cargo fmt --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

Normal tests do not invoke live Codex.

---

# 46. Final Design Contract

Relay runtime configuration should feel like:

```text
Provider
  Codex

Safety profile
  Implement

AI tuning
  Model       GPT-5.3-Codex
  Reasoning   High

Execution bounds
  Duration    30m
  Repairs     2
```

not:

```text
choose "Implement"
→ model/reasoning permanently chosen for you
```

The permanent rule is:

> **Profiles define the trusted execution envelope. The orchestrator may tune provider-native intelligence settings inside that envelope.**

For Codex specifically:

> **Model and reasoning effort are first-class Relay controls, frozen per execution and translated into explicit Codex invocation overrides.**
