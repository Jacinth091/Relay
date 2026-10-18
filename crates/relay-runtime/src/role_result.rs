//! The typed role-result contract: what a provider process must publish on
//! stdout for its role attempt to be accepted.
//!
//! Process exit success alone is never sufficient. Exit status says a process
//! ended, not that the role produced anything usable, so the runtime layer owns
//! exactly one parser and one vocabulary for the typed result. Provider
//! adapters and the v2 orchestrator share it rather than each re-deriving
//! "did the role publish something?" from process status.
//!
//! Wire form is one JSON object on its own line of stdout:
//!
//! ```text
//! {"role_result":"implemented","detail":"..."}
//! {"role_result":"verification_passed"}
//! {"role_result":"verification_failed","reason":"..."}
//! ```

use serde::{Deserialize, Serialize};

use crate::RuntimeObservation;

/// The typed result a role process must publish on stdout for its attempt to
/// be accepted. Process exit success alone is never sufficient: exit status
/// says a process ended, not that the role produced anything usable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role_result", rename_all = "snake_case")]
pub enum RoleResultPayload {
    Implemented { detail: String },
    VerificationPassed,
    VerificationFailed { reason: String },
}

impl RoleResultPayload {
    /// Stable name used in diagnostics and in "wrong role" rejections.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Implemented { .. } => "implemented",
            Self::VerificationPassed => "verification_passed",
            Self::VerificationFailed { .. } => "verification_failed",
        }
    }
}

/// Why an observation could not be accepted as a typed role result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultDefect {
    /// The process published no recognizable result object at all.
    Missing,
    /// A result object was present but violated the contract, or belonged to a
    /// different role than the one that produced it.
    Malformed(String),
}

/// A role result as published, kept alongside the exact bytes the process
/// wrote so the resulting artifact stores the observation rather than a
/// summary the orchestrator invented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedRoleResult {
    pub payload: RoleResultPayload,
    pub raw: String,
}

/// Extracts the typed role result a process published on stdout.
///
/// Only a well-formed result envelope counts. A process that exited with no
/// envelope yields [`ResultDefect::Missing`]; an envelope that violates the
/// contract yields [`ResultDefect::Malformed`].
pub fn observed_role_result(
    observation: &RuntimeObservation,
) -> Result<PublishedRoleResult, ResultDefect> {
    let stdout = String::from_utf8_lossy(&observation.stdout_chunk);

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("role_result").is_none() {
            continue;
        }

        return match serde_json::from_value::<RoleResultPayload>(value) {
            Ok(payload) => Ok(PublishedRoleResult {
                payload,
                raw: line.to_owned(),
            }),
            Err(error) => Err(ResultDefect::Malformed(format!(
                "result object does not satisfy the role result contract: {error}"
            ))),
        };
    }

    Err(ResultDefect::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProcessStatus;

    fn observation(stdout: &str) -> RuntimeObservation {
        RuntimeObservation {
            handle_id: "handle-role-result".to_owned(),
            status: ProcessStatus::ExitedSuccess { exit_code: 0 },
            stdout_chunk: stdout.as_bytes().to_vec(),
            stderr_chunk: vec![],
            total_bytes_captured: stdout.len(),
        }
    }

    #[test]
    fn accepts_a_published_implemented_result_and_preserves_raw_bytes() {
        let raw = r#"{"role_result":"implemented","detail":"done"}"#;
        let published = observed_role_result(&observation(raw)).expect("valid envelope");
        assert_eq!(
            published.payload,
            RoleResultPayload::Implemented {
                detail: "done".to_owned()
            }
        );
        assert_eq!(published.payload.kind(), "implemented");
        assert_eq!(published.raw, raw);
    }

    #[test]
    fn ignores_non_result_lines_and_reports_missing() {
        let defect = observed_role_result(&observation("building...\n{\"other\":1}\n"))
            .expect_err("no envelope");
        assert_eq!(defect, ResultDefect::Missing);
    }

    #[test]
    fn rejects_an_envelope_that_violates_the_contract() {
        let defect = observed_role_result(&observation("{\"role_result\":\"bogus\"}\n"))
            .expect_err("malformed envelope");
        match defect {
            ResultDefect::Malformed(reason) => {
                assert!(
                    reason.contains("role result contract"),
                    "reason must name the contract, got {reason:?}"
                );
            }
            other => panic!("expected a malformed defect, got {other:?}"),
        }
    }
}
