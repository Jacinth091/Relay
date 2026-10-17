@echo off
setlocal

rem ===========================================================================
rem  Relay focused check script (portable)
rem
rem  Run this from a Developer Command Prompt / Developer PowerShell for Visual
rem  Studio so the MSVC linker (link.exe) and Windows SDK are on PATH.
rem  The script deliberately contains no absolute paths and does not try to
rem  locate a Visual Studio installation: that location is machine-specific.
rem
rem  It uses its own directory, and stops at the first failure.
rem ===========================================================================

cd /d "%~dp0"

echo === Relay focused checks ===
echo.

echo === Formatting ===
cargo fmt --all -- --check || goto :error
echo.

echo === Clippy (workspace, warnings as errors) ===
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings || goto :error
echo.

echo === v2 suites: relay-core ===
cargo test -p relay-core --test phase02_trust_security || goto :error
cargo test -p relay-core --test phase03_workflow_and_intent || goto :error
cargo test -p relay-core --test phase04_candidate_lease_artifact || goto :error
cargo test -p relay-core --test phase05_tracer_bullet || goto :error
cargo test -p relay-core --test phase09_exchange_capabilities || goto :error
cargo test -p relay-core --test phase10_delegation_budgets || goto :error
cargo test -p relay-core --test phase11_multi_project_scheduler || goto :error
cargo test -p relay-core --test phase12_daemon_clients || goto :error
cargo test -p relay-core --test phase13_privileged_actions || goto :error
cargo test -p relay-core --test phase14_semantic_decisions || goto :error
cargo test -p relay-core --test phase15_integrated_recovery || goto :error
echo.

echo === v2 suites: relay-runtime ===
cargo test -p relay-runtime --test phase06_runtime_backplane || goto :error
cargo test -p relay-runtime --test phase07_herdr_backplane || goto :error
cargo test -p relay-runtime --test phase08_provider_adapters || goto :error
echo.

echo === Process-boundary integration tests: relay-tui ===
cargo test -p relay-tui --test daemon_cli || goto :error
cargo test -p relay-tui --test serve_mcp || goto :error
echo.

echo === All focused checks passed ===
echo For the full gate, run: cargo test --workspace --locked
exit /b 0

:error
echo.
echo *** CHECK FAILED (exit code %errorlevel%) ***
exit /b %errorlevel%
