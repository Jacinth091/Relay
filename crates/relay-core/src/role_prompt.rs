use crate::ApprovedIntent;

/// Builds the one-shot provider prompt from the intent frozen on the workflow
/// run and the role's typed result contract.
pub fn role_prompt(
    role: &str,
    intent: &ApprovedIntent,
    remediation_reason: Option<&str>,
) -> String {
    let result_contract = match role {
        "implementer" => {
            "When finished, print exactly one line containing only one of these JSON objects: {\"role_result\":\"implemented\",\"detail\":\"implementation summary\"}."
        }
        "verifier" => {
            "When finished, print exactly one line containing only one of these JSON objects: {\"role_result\":\"verification_passed\"} or {\"role_result\":\"verification_failed\",\"reason\":\"failure reason\"}."
        }
        _ => "Do not publish a role result for an unsupported role.",
    };
    let remediation = match (role, remediation_reason) {
        ("implementer", Some(reason)) => format!("\n\nVerifier failure reason:\n{reason}"),
        _ => String::new(),
    };

    format!(
        "Role: {role}\n\nObjective:\n{}\n\nAcceptance criteria:\n{}\n\nConstraints:\n{}\n\nNon-goals:\n{}{}\n\nResult contract:\n{result_contract}",
        intent.objective,
        format_items(&intent.acceptance_criteria),
        format_items(&intent.constraints),
        format_items(&intent.non_goals),
        remediation,
    )
}

fn format_items(items: &[String]) -> String {
    if items.is_empty() {
        "- (none)".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApprovedIntentId, TaskId};

    fn intent() -> ApprovedIntent {
        ApprovedIntent {
            id: ApprovedIntentId("intent-role-prompt".to_owned()),
            task_id: TaskId::generate(),
            generation: 1,
            objective: "Implement the approved search filter".to_owned(),
            acceptance_criteria: vec![
                "Filters by owner".to_owned(),
                "Preserves sort order".to_owned(),
            ],
            constraints: vec!["No network access".to_owned()],
            non_goals: vec!["Do not redesign the screen".to_owned()],
            approved_by: "operator".to_owned(),
            approved_at: "now".to_owned(),
            content_hash: "hash".to_owned(),
        }
    }

    #[test]
    fn implementer_prompt_contains_frozen_intent_and_implementer_contract() {
        let prompt = role_prompt("implementer", &intent(), None);

        assert!(prompt.contains("Implement the approved search filter"));
        assert!(prompt.contains("Filters by owner"));
        assert!(prompt.contains("Preserves sort order"));
        assert!(prompt.contains("No network access"));
        assert!(prompt.contains("Do not redesign the screen"));
        assert!(prompt.contains("\"role_result\":\"implemented\""));
        assert!(!prompt.contains("verification_passed"));
        assert!(!prompt.contains("verification_failed"));
    }

    #[test]
    fn verifier_prompt_contains_only_verifier_result_kinds() {
        let prompt = role_prompt("verifier", &intent(), None);

        assert!(prompt.contains("\"role_result\":\"verification_passed\""));
        assert!(prompt.contains("\"role_result\":\"verification_failed\""));
        assert!(!prompt.contains("\"role_result\":\"implemented\""));
    }

    #[test]
    fn remediation_prompt_includes_verifier_failure_reason() {
        let prompt = role_prompt(
            "implementer",
            &intent(),
            Some("missing regression coverage"),
        );

        assert!(prompt.contains("Verifier failure reason:\nmissing regression coverage"));
    }
}
