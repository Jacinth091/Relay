# Public Repository Readiness Checklist

**Scope:** Prepare Relay for public, experimental hobby/development use. This checklist is a release gate, **not** a claim that any item is complete.

Changing a GitHub repository from private to public exposes more than the current files. Do **not** change visibility until the owner explicitly completes the relevant checks.

## 1. Privacy, security, and history (blocking)

- [ ] Inventory **all** Git refs: default branch, staging, feature branches, tags, past commits, and PR discussions.
- [ ] Scan the **entire reachable Git history**, not merely the working tree, for tokens, keys, credentials, emails, private links, internal project data, personal notes, database files, and accidentally tracked artifacts. Use a history-aware secrets scanner locally and review false positives.
- [ ] Review commit author names/emails, commit messages, issue/PR comments, uploaded attachments, and past GitHub Actions logs/artifacts. Public visibility also exposes Actions history and logs.
- [ ] Review copied/vendor code, documentation, images, and other assets for license compatibility and attribution; update `THIRD_PARTY_NOTICES.md` if required.
- [ ] If secrets were ever committed, **revoke/rotate them first**. Coordinate any necessary history cleanup and branch/reference changes before publication. Removing a file in the latest commit or adding `.gitignore` is insufficient.
- [ ] Confirm no private workspaces, task handoff payloads, local SQLite files, `.relay/` state, or user configuration are present in published refs.

## 2. What is actually being published (blocking)

- [ ] Choose the public baseline. `main` can lag the `staging`, M9, and architecture rebaseline branches; do not imply unmerged work is implemented or stable.
- [ ] Reconcile public `README.md`, `docs/installation.md`, roadmap, and branch history with tested source. Mark future plans as such.
- [ ] Choose and approve the license with all relevant rights-holders. **MIT is proposed** to match Herdr Loop Skill, but it permits commercial as well as hobby use.
- [ ] Confirm the project name, screenshots, examples, acknowledgments, and links are appropriate to share.
- [ ] Ensure contributors can build/test from source without credentials or a proprietary hosted service.

## 3. Quality and community

- [ ] Merge the public-readiness changes after review. Confirm Linux and Windows CI passes on the exact merged candidate.
- [ ] Test a fresh clone from a new directory using `docs/DEVELOPMENT.md`.
- [ ] Keep bug-report and feature-request templates short; warn against publishing secrets.
- [ ] Set repository description, topics, and optional website link to accurately reflect experimental status.
- [ ] Enable suitable branch rules for `main`: PR reviews/checks, block force-push and deletion, and restrict release-tag creation if releases are adopted.
- [ ] Enable appropriate dependency/security alerts and **private vulnerability reporting** once public; confirm `SECURITY.md` matches the enabled reporting path.
- [ ] Review Actions and fork PR permissions. Avoid privileged workflows that execute untrusted fork code with write access or production credentials.

## 4. Visibility decision and aftercare

- [ ] Re-check all above items and explicitly approve the private-to-public switch.
- [ ] In GitHub repository Settings > Danger Zone > Change repository visibility, choose **Public** and complete confirmation.
- [ ] Confirm the public README, LICENSE, community files, and workflow checks render as intended.
- [ ] Consider an explicitly labeled prerelease only after the source and distribution are tested; no automatic binary publishing is required for an experimental project.
- [ ] Keep any personal/project handoff mailbox repository **private**. A public Relay **source** repository is not an acceptable default mailbox for private implementation tasks.

## Release posture

Relay is a personal experiment. Public source availability is not a promise of production readiness, free hosted services, full provider support, zero security risk, or maintenance obligations.

GitHub reference: [repository visibility consequences](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/managing-repository-settings/setting-repository-visibility) and [sensitive data removal](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository).
