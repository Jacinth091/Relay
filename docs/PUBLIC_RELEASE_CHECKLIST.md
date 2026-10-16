# Public Repository Readiness Checklist

**Scope:** prepare Relay for public, experimental hobby/development use.
**Status:** a release gate — **not** a claim that any item is complete.

Changing a GitHub repository from private to public exposes far more than the
default-branch files: all branches, tags, commit history and author metadata,
pull-request discussions, uploaded attachments, and Actions run logs and
artifacts become visible. Do **not** change visibility until the owner has
explicitly completed the relevant gates below.

Legend: `[blocking]` must be resolved before publication · `[owner]` requires an
explicit owner decision · `[post]` is done immediately after publication.

---

## 1. Licensing `[blocking] [owner]`

- [ ] Owner has **approved MIT** for the repository and confirms it is the
      intended grant. MIT permits commercial use and redistribution; it is
      **not** a "hobby-only" or non-commercial license.
- [ ] Copyright holder line in `LICENSE` is correct for all rights-holders.
- [ ] Every contributor/author with rights to committed material is compatible
      with the MIT grant (including any material written before `LICENSE` existed).
- [ ] No dependency or vendored asset imposes a conflicting obligation on the
      distributed source (dependencies keep their own licenses; see
      `THIRD_PARTY_NOTICES.md`).

## 2. History, privacy, and secrets audit `[blocking]`

- [ ] Inventory **all** refs: default branch, `staging`, feature branches, tags,
      remote-tracking branches, and the full reachable commit history.
- [ ] Run a **history-aware** secret scan (`--all`, not just the working tree)
      and review every hit as a false positive or a real finding.
- [ ] Manually review filenames and content categories that token scanners miss:
      `.env*`, `*.db`/`*.sqlite*`, `.relay/`, logs, archives, data extracts,
      installers, private task handoffs, personal notes.
- [ ] Review commit author/committer names and emails for anything the owner does
      not want public.
- [ ] Review commit messages, PR/issue discussions, review bodies, and uploaded
      attachments — these are **not** in a normal local clone.
- [ ] Review Actions run logs and artifacts; public visibility exposes them.
- [ ] Confirm no private workspace paths, machine names, or credentials in
      tracked scripts or fixtures.
- [ ] If any live secret was ever committed: **revoke/rotate it first**. Removing
      the file in the latest commit or adding `.gitignore` is **not** sufficient
      for material already in history. History rewriting requires separate
      authorization and coordination.
- [ ] Owner has accepted any unavoidable historical disclosure.

## 3. Third-party attribution `[blocking]`

- [ ] `ACKNOWLEDGMENTS.md` lists inspirations and is accurate (ideas, not copied
      code).
- [ ] `THIRD_PARTY_NOTICES.md` reflects any copied/vendored source, docs, or
      assets, with required notices and license text.
- [ ] No third-party code was copied without checking its license.
- [ ] Project name, screenshots, examples, and links are appropriate to publish.

## 4. Documentation and status truthfulness `[blocking]`

- [ ] `README.md` describes Relay v2 as **experimental** with no support
      commitment, and its Today / Partial / Planned table matches the code at the
      candidate revision.
- [ ] `docs/v2_architecture_rebaseline/ROADMAP.md` is the authoritative status and
      does not mark a phase complete without a proven production exit gate.
- [ ] Historical v1 material remains historical (archived, labeled) and is not
      rewritten to look like v2.
- [ ] Active docs contain no broken relative links to moved/removed files.
- [ ] No doc advertises a CLI subcommand, installer, or integration that does not
      exist in the source at the candidate revision.
- [ ] `AGENTS.md` invariants and the public docs agree with ADR-0020 ownership.

## 5. Build, tests, and CI `[blocking]`

- [ ] `cargo fmt --all -- --check`,
      `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`,
      and `cargo test --workspace --locked` pass at the **exact** candidate SHA.
- [ ] A fresh clone builds and the documented commands work from a clean
      directory on at least Windows and Linux.
- [ ] `.github/workflows/ci.yml` is read-only (`permissions: contents: read`), has
      no secrets, no privileged `pull_request_target`, and makes no provider calls.
- [ ] CI does not require credentials, a hosted service, or paid AI capacity.
- [ ] `run_checks.bat` contains no absolute machine paths (it relies on a
      Developer Command Prompt for `link.exe`).
- [ ] No tracked secrets, local state, `.env` files, SQLite databases, or `.relay/`
      privileged state were introduced by the change.

## 6. Chosen public baseline `[owner]`

- [ ] Owner has decided which branch becomes the public default. The v2 work
      branch diverges from `main`; promotion should go through the normal
      reviewed `staging` → `main` path, with its own tests and conflict
      resolution. Do not flip the default branch as a side effect of a docs PR.
- [ ] Public docs match the branch that will actually be visible.

## 7. GitHub settings and rules `[blocking] [owner]`

- [ ] Repository description and topics accurately state experimental status.
- [ ] Branch rules protect the default branch: require PR review and passing
      checks; block force-push and deletion; restrict release-tag creation if
      releases are adopted.
- [ ] Review Actions permissions and fork-PR behavior (approve first-time
      contributors, restrict workflow write access).
- [ ] Enable dependency/security alerts as appropriate.
- [ ] Note: converting a repository from private to public can **disable push
      rulesets** and exposes Actions history. Re-check and restore rule
      protections after the switch.

## 8. Visibility decision `[blocking] [owner]`

- [ ] Owner re-checks every blocking gate above and explicitly approves the
      private → public switch.
- [ ] Visibility is changed only in GitHub Settings → Danger Zone → Change
      repository visibility (owner action).

## 9. Post-publication `[post]`

- [ ] Confirm the public README, LICENSE, CONTRIBUTING, SECURITY, issue forms,
      PR template, and CI render/run as intended.
- [ ] Enable **private vulnerability reporting** (available only for public
      repositories) and confirm `SECURITY.md` matches the enabled path.
- [ ] Re-enable/verify branch rules and rulesets.
- [ ] Verify Actions runs are not exposing unexpected logs or artifacts.
- [ ] Keep any personal handoff mailbox repository **private**. A public Relay
      source repository is not a mailbox for private implementation tasks.
- [ ] Optional: a prerelease may be considered only after source and
      distribution are tested end to end. No automatic binary publishing is
      required for an experimental project.

---

## Release posture

Relay is a personal experiment. Public source availability is not a promise of
production readiness, a hosted service, full provider support, zero security
risk, maintenance obligations, or a response time.

## References

- GitHub — [Setting repository visibility](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/managing-repository-settings/setting-repository-visibility)
- GitHub — [Removing sensitive data from a repository](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository)
- GitHub — [Configuring private vulnerability reporting](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository)
