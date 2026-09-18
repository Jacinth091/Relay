@echo off
call "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvarsall.bat" x64
cd /d "C:\Users\PCC\Desktop\My Projects\Relay"

echo === Running Phase 09, 10, 11, 12, 13, 14, 15 Test Suites ===
cargo test -p relay-core --test phase09_exchange_capabilities
cargo test -p relay-core --test phase10_delegation_budgets
cargo test -p relay-core --test phase11_multi_project_scheduler
cargo test -p relay-core --test phase12_daemon_clients
cargo test -p relay-core --test phase13_privileged_actions
cargo test -p relay-core --test phase14_semantic_decisions
cargo test -p relay-core --test phase15_integrated_recovery

echo === Running cargo fmt check ===
cargo fmt --check

echo === Running cargo clippy ===
cargo clippy --workspace --all-targets --all-features -- -D warnings
