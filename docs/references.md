# Primary Technical References

These links are included so future contributors can verify architectural assumptions against primary documentation.

## Rust / Cargo

- Cargo workspaces: https://doc.rust-lang.org/stable/cargo/reference/workspaces.html
- Rust book: https://doc.rust-lang.org/book/

## Ratatui

- Concepts: https://ratatui.rs/concepts/
- Application patterns: https://ratatui.rs/concepts/application-patterns/

## Tokio

- Tokio: https://tokio.rs/
- Tokio process module: https://docs.rs/tokio/latest/tokio/process/

## Serde

- https://serde.rs/

## SQLx

- https://docs.rs/sqlx/latest/sqlx/

## Tauri

- https://v2.tauri.app/start/
- Frontend configuration: https://v2.tauri.app/start/frontend/

## Axum

- https://docs.rs/axum/latest/axum/

## Note

Dependency APIs and versions change.

Relay should pin actual versions in `Cargo.lock` and review release notes during upgrades rather than treating this document as a version lock.

## Architectural Research References

- AIF Handoff: https://github.com/lee-to/aif-handoff

This is an inspiration/research reference, not a dependency. See `ACKNOWLEDGMENTS.md` and `THIRD_PARTY_NOTICES.md`.
\n\n## Planner host integration references\n\nVerified for the provider-adapter baseline on 2026-08-13:\n\n- ChatGPT Skills: https://help.openai.com/en/articles/20001066\n- Claude Code Skills: https://code.claude.com/docs/en/skills\n- OpenCode Skills: https://opencode.ai/v2/docs/skills\n- GitHub Copilot Agent Skills: https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-cloud-agent/add-skills\n\nThese are provider-owned integration surfaces and can change independently of Relay. Re-verify before changing installation automation.\n