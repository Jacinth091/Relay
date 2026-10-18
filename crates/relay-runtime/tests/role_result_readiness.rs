//! R12-11: provider exit never counts as a role result.
//!
//! A provider process that exits successfully is `ResultPublished` only when
//! it actually published a well-formed typed role result envelope on stdout.
//! Exiting 0 with no envelope, or with an unusable one, is a provider error --
//! never a published result. These cases are asserted through the public
//! `ProviderAdapter` seam for every adapter that shares the process
//! interpretation helper.

use relay_runtime::{
    CodexProviderAdapter, MockProviderAdapter, ProcessStatus, ProviderAdapter,
    ProviderCapabilities, ProviderReadiness, RuntimeObservation,
};

fn implementer_caps() -> ProviderCapabilities {
    ProviderCapabilities {
        supported_roles: ["implementer"].into_iter().map(str::to_owned).collect(),
        filesystem_write: true,
        network: false,
        streaming_output: true,
        cancellation: true,
        supported_models: vec!["default".to_owned()],
    }
}

/// Every adapter that derives readiness from `ProcessStatus` + stdout.
fn adapters() -> Vec<(&'static str, Box<dyn ProviderAdapter>)> {
    let mut adapters: Vec<(&'static str, Box<dyn ProviderAdapter>)> = Vec::new();
    adapters.push((
        "mock",
        Box::new(MockProviderAdapter::new("mock", implementer_caps())),
    ));
    adapters.push(("codex", Box::new(CodexProviderAdapter::new())));
    adapters
}

fn observation(status: ProcessStatus, stdout: &str) -> RuntimeObservation {
    RuntimeObservation {
        handle_id: "handle-r12-11".to_owned(),
        status,
        stdout_chunk: stdout.as_bytes().to_vec(),
        stderr_chunk: vec![],
        total_bytes_captured: stdout.len(),
    }
}

#[test]
fn exit_zero_without_an_envelope_is_not_a_published_result() {
    for (name, adapter) in adapters() {
        let obs = observation(ProcessStatus::ExitedSuccess { exit_code: 0 }, "");
        let readiness = adapter.interpret_observation(&obs);

        assert!(
            !matches!(readiness, ProviderReadiness::ResultPublished),
            "{name}: exit 0 with no envelope must not be ResultPublished"
        );
        match readiness {
            ProviderReadiness::ProviderError { reason } => assert!(
                reason.contains("without publishing"),
                "{name}: reason must say it exited without a result, got {reason:?}"
            ),
            other => panic!("{name}: expected a provider error, got {other:?}"),
        }
    }
}

#[test]
fn exit_zero_with_a_malformed_envelope_is_not_a_published_result() {
    for (name, adapter) in adapters() {
        let obs = observation(
            ProcessStatus::ExitedSuccess { exit_code: 0 },
            "{\"role_result\":\"bogus\"}\n",
        );
        let readiness = adapter.interpret_observation(&obs);

        assert!(
            !matches!(readiness, ProviderReadiness::ResultPublished),
            "{name}: a malformed envelope must not be ResultPublished"
        );
        match readiness {
            ProviderReadiness::ProviderError { reason } => assert!(
                reason.contains("contract"),
                "{name}: reason must mention the result contract, got {reason:?}"
            ),
            other => panic!("{name}: expected a provider error, got {other:?}"),
        }
    }
}

#[test]
fn exit_zero_with_a_valid_envelope_is_a_published_result() {
    for (name, adapter) in adapters() {
        let obs = observation(
            ProcessStatus::ExitedSuccess { exit_code: 0 },
            "building...\n{\"role_result\":\"implemented\",\"detail\":\"done\"}\n",
        );
        assert_eq!(
            adapter.interpret_observation(&obs),
            ProviderReadiness::ResultPublished,
            "{name}: a well-formed envelope after exit 0 is a published result"
        );
    }
}

#[test]
fn non_zero_exit_is_an_error_even_with_a_valid_envelope() {
    for (name, adapter) in adapters() {
        let obs = observation(
            ProcessStatus::ExitedFailure { exit_code: Some(1) },
            "{\"role_result\":\"implemented\",\"detail\":\"done\"}\n",
        );
        assert!(
            matches!(
                adapter.interpret_observation(&obs),
                ProviderReadiness::ProviderError { .. }
            ),
            "{name}: a failed exit stays an error even when an envelope is present"
        );
    }
}
