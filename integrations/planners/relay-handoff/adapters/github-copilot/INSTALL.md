# GitHub Copilot Adapter Installation

Status: **experimental first-party adapter; native mechanism verified, Relay end-to-end validation pending**.

## Native mechanism

GitHub Copilot Agent Skills (`SKILL.md`).

## Project installation

Copy `relay-handoff` to one of the supported project skill roots, preferably:

```text
<project>/.github/skills/relay-handoff/
```

GitHub also documents `.agents/skills/` and `.claude/skills/` as compatible project skill roots.

## Personal installation

Copy the directory to:

```text
~/.copilot/skills/relay-handoff/
```

or the cross-agent location:

```text
~/.agents/skills/relay-handoff/
```

## GitHub CLI skill management

GitHub CLI also provides `gh skill` commands for skill discovery/installation/publishing in public preview. Relay must not require this preview feature for basic installation.

## Relay submission

Prefer a supported Relay Direct tool when the active Copilot surface can call Relay. GitHub Issue creation remains a useful compatibility transport when authorized, but it is not required after a direct Relay handoff. Relay may create an optional GitHub binding afterward according to local configuration.

## Safety

Do not pre-approve shell/bash in `allowed-tools` for this Skill. Relay Handoff can prepare tasks without unrestricted shell permission.

## Official reference

https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-cloud-agent/add-skills
