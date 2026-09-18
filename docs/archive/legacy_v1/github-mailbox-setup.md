# GitHub Mailbox Setup

> [!NOTE]
> This describes Relay's bootstrap GitHub transport for planner hosts that cannot reach Relay directly. Users with a supported Relay Direct/MCP connection may skip the mailbox entirely.

## Purpose

Before a planner can connect directly to a user's local Relay instance, a private GitHub repository can act as a durable shared inbox for approved handoffs.

```text
Planner
  -> user's private Relay mailbox repository
  -> Relay GitHub ingress
  -> HandoffTask v1
  -> trusted Project Registry
  -> correct local project
```

The mailbox is only a transport into Relay. It is not the canonical Relay task database and it is not the target project repository.

## Each user owns their mailbox

Every Relay user or team that chooses the GitHub mailbox transport should create or select **their own private GitHub repository** for Relay handoffs.

Do not configure a public example, a Relay maintainer's repository, or another user's mailbox as the default destination.

The repository name is user-defined. Examples include:

```text
<your-account>/relay-handoff
<your-account>/relay-mailbox
<your-account>/ai-handoff
```

`ai-codex-handoff` is only an example/reference name from Relay's early development. It is not a shared service operated for Relay users.

## Why private is the default recommendation

Handoff Issues can contain implementation goals, requirements, constraints, verification expectations, architecture context, and clarification packets. A private repository reduces the risk of unintentionally publishing planning context for private or unreleased work.

Relay should make repository visibility obvious during setup and warn before a public repository is selected as a mailbox.

A user may deliberately choose a public repository if that matches their workflow, but Relay should not silently default to one.

## One mailbox can serve many Relay projects

The mailbox is shared across the user's configured Relay projects.

For example:

```text
private Relay mailbox
  |- Issue #101: project = "practicore"
  |- Issue #102: project = "jci-backend"
  `- Issue #103: project = "relay"
```

Every HandoffTask must identify exactly one logical Relay project.

Relay reads the `project` key and resolves it through the trusted local Project Registry. The planner does not provide local filesystem paths or trusted repository configuration.

## Intended setup flow

When the user selects **GitHub mailbox** as the planner submission transport, Relay should guide them through:

1. Create or select a GitHub repository owned by the user or their organization.
2. Prefer private visibility for the mailbox.
3. Ensure GitHub Issues are available for the repository.
4. Authenticate Relay's local GitHub integration.
5. Configure the mailbox repository in trusted local Relay configuration.
6. If the selected planner will submit through GitHub, separately ensure that planner has authorized write access to that same mailbox.
7. Test mailbox access before marking the transport ready.

The exact UI and commands are implementation details and should not be documented as working until they are built and tested.

## Trusted configuration

The mailbox destination belongs in trusted setup/configuration, not in each incoming HandoffTask.

Conceptually:

```text
submission transport: provider-github
mailbox repository: <user-owned-private-repository>
```

The planner adapter may be configured with that destination so it knows where to deliver bootstrap handoffs, but it must never guess the repository or fall back to a maintainer-owned example.

## Relationship to project-specific Issues

A mailbox Issue may be only an ingress envelope.

After Relay imports the task, local policy may optionally create a project-specific GitHub Issue as an external binding:

```text
user mailbox Issue
      -> Relay
      -> project = practicore
      -> optional PractiCore repository Issue
```

Once Relay Direct/MCP is available, the normal path may skip the mailbox completely:

```text
Planner
  -> Relay Direct/MCP
  -> Relay
  -> optional project-specific GitHub Issue
```

## No mailbox requirement for direct users

A user who has a supported direct planner-to-Relay transport should not be forced to create a GitHub mailbox.

The mailbox is the default/reference bootstrap transport when automatic direct submission is unavailable. Local file/paste remains the universal manual fallback.
