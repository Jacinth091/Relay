//! Relay Architecture v2 - Phase 03: Domain Contracts & Workflow Engine.
//!
//! Introduces the semantic heart of v2:
//! - `ApprovedIntent`: immutable user intent (scope changes create a new generation);
//! - `ExecutionGrant`: explicit human approval to execute under an `ExecutionBudget`;
//! - `WorkflowDefinition` / `WorkflowRun`: graph-based workflow and execution lifecycle;
//! - `RoleDefinition` / `RoleInstance` / `RoleAttempt`: semantic roles separated from provider attempts;
//! - `ClarificationRequest` / `DecisionRequest` / `ScopeExpansionRequest`: typed human escalations;
//! - `ConfigSnapshot`: immutable run-time configuration fingerprint.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::TaskId;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("workflow validation error: {reason}")]
    Validation { reason: String },
    #[error("workflow run error: {0}")]
    RunError(String),
    #[error("budget exhausted: {reason}")]
    BudgetExhausted { reason: String },
    #[error("role attempt error: {0}")]
    AttemptError(String),
    #[error("intent is locked to generation {0}; scope modification requires new generation")]
    IntentLocked(u32),
}

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ApprovedIntentId(pub String);

impl ApprovedIntentId {
    pub fn generate() -> Self {
        Self(format!("intent-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for ApprovedIntentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ExecutionGrantId(pub String);

impl ExecutionGrantId {
    pub fn generate() -> Self {
        Self(format!("grant-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for ExecutionGrantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowDefinitionId(pub String);

impl WorkflowDefinitionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for WorkflowDefinitionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkflowRunId(pub String);

impl WorkflowRunId {
    pub fn generate() -> Self {
        Self(format!("wfrun-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for WorkflowRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoleDefinitionId(pub String);

impl RoleDefinitionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for RoleDefinitionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoleInstanceId(pub String);

impl RoleInstanceId {
    pub fn generate(role_id: &RoleDefinitionId) -> Self {
        Self(format!(
            "role-{}-{}",
            role_id.0,
            uuid::Uuid::new_v4().simple()
        ))
    }
}

impl std::fmt::Display for RoleInstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RoleAttemptId(pub String);

impl RoleAttemptId {
    pub fn generate(instance_id: &RoleInstanceId, attempt_no: u32) -> Self {
        Self(format!("{}-att{}", instance_id.0, attempt_no))
    }
}

impl std::fmt::Display for RoleAttemptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConfigSnapshotId(pub String);

impl ConfigSnapshotId {
    pub fn generate() -> Self {
        Self(format!("cfg-{}", uuid::Uuid::new_v4().simple()))
    }
}

impl std::fmt::Display for ConfigSnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// 6. ApprovedIntent
// ---------------------------------------------------------------------------

/// Immutable approved user intent. Scope change creates a new generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedIntent {
    pub id: ApprovedIntentId,
    pub task_id: TaskId,
    pub generation: u32,
    pub objective: String,
    pub acceptance_criteria: Vec<String>,
    pub constraints: Vec<String>,
    pub non_goals: Vec<String>,
    pub approved_by: String,
    pub approved_at: String,
    pub content_hash: String,
}

impl ApprovedIntent {
    pub fn new(
        task_id: TaskId,
        generation: u32,
        objective: String,
        acceptance_criteria: Vec<String>,
        constraints: Vec<String>,
        non_goals: Vec<String>,
        approved_by: String,
    ) -> Self {
        let content_hash = compute_intent_hash(
            &task_id,
            generation,
            &objective,
            &acceptance_criteria,
            &constraints,
            &non_goals,
        );
        Self {
            id: ApprovedIntentId::generate(),
            task_id,
            generation,
            objective,
            acceptance_criteria,
            constraints,
            non_goals,
            approved_by,
            approved_at: now_timestamp(),
            content_hash,
        }
    }

    /// Creates a successor generation when scope is modified.
    ///
    /// The current intent is NEVER mutated in place (invariant 5 & 22).
    pub fn create_next_generation(
        &self,
        new_objective: String,
        new_acceptance_criteria: Vec<String>,
        new_constraints: Vec<String>,
        approved_by: String,
    ) -> Self {
        self.create_next_generation_with_non_goals(
            new_objective,
            new_acceptance_criteria,
            new_constraints,
            self.non_goals.clone(),
            approved_by,
        )
    }

    /// Creates a successor generation with updated non-goals.
    pub fn create_next_generation_with_non_goals(
        &self,
        new_objective: String,
        new_acceptance_criteria: Vec<String>,
        new_constraints: Vec<String>,
        new_non_goals: Vec<String>,
        approved_by: String,
    ) -> Self {
        Self::new(
            self.task_id,
            self.generation + 1,
            new_objective,
            new_acceptance_criteria,
            new_constraints,
            new_non_goals,
            approved_by,
        )
    }
}

fn compute_intent_hash(
    task_id: &TaskId,
    generation: u32,
    objective: &str,
    acceptance_criteria: &[String],
    constraints: &[String],
    non_goals: &[String],
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(task_id.to_string().as_bytes());
    hasher.update(generation.to_le_bytes());
    hasher.update(objective.as_bytes());
    for ac in acceptance_criteria {
        hasher.update(ac.as_bytes());
    }
    for c in constraints {
        hasher.update(c.as_bytes());
    }
    for ng in non_goals {
        hasher.update(ng.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// 20. ExecutionBudget
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionBudget {
    pub max_role_activations: u32,
    pub max_attempts_per_role: u32,
    pub max_delegations: u32,
    pub max_delegation_depth: u32,
    pub max_concurrent_attempts: u32,
    pub max_wall_clock_secs: Option<u64>,
}

impl Default for ExecutionBudget {
    fn default() -> Self {
        Self {
            max_role_activations: 10,
            max_attempts_per_role: 3,
            max_delegations: 5,
            max_delegation_depth: 2,
            max_concurrent_attempts: 2,
            max_wall_clock_secs: Some(600),
        }
    }
}

// ---------------------------------------------------------------------------
// 7. ExecutionGrant
// ---------------------------------------------------------------------------

/// Execution grant created upon explicit human run approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionGrant {
    pub id: ExecutionGrantId,
    pub task_id: TaskId,
    pub approved_intent_id: ApprovedIntentId,
    pub approved_intent_generation: u32,
    pub workflow_definition_id: WorkflowDefinitionId,
    pub config_snapshot_id: ConfigSnapshotId,
    pub execution_budget: ExecutionBudget,
    pub allowed_delegation_classes: Vec<RoleDefinitionId>,
    pub granted_by: String,
    pub granted_at: String,
    pub revoked_at: Option<String>,
}

impl ExecutionGrant {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }

    pub fn revoke(&mut self) {
        self.revoked_at = Some(now_timestamp());
    }
}

// ---------------------------------------------------------------------------
// 8. WorkflowDefinition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransitionCondition {
    Always,
    RoleAttemptSucceeded,
    RoleAttemptFailed,
    CandidateCreated,
    ArtifactProduced { kind: String },
    RemediationNeeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowNodeDefinition {
    pub id: String,
    pub role_id: RoleDefinitionId,
    #[serde(default)]
    pub consumed_artifact_types: Vec<String>,
    #[serde(default)]
    pub produced_artifact_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowTransitionDefinition {
    pub from_node: String,
    pub to_node: String,
    pub condition: TransitionCondition,
    #[serde(default)]
    pub required_artifact_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub id: WorkflowDefinitionId,
    pub version: u32,
    #[serde(default)]
    pub roles: Vec<RoleDefinition>,
    pub nodes: Vec<WorkflowNodeDefinition>,
    pub transitions: Vec<WorkflowTransitionDefinition>,
    pub default_budget: ExecutionBudget,
    pub content_hash: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DfsState {
    Unvisited,
    Visiting,
    Visited,
}

impl WorkflowDefinition {
    pub fn validate(&self) -> Result<(), WorkflowError> {
        if self.nodes.is_empty() {
            return Err(WorkflowError::Validation {
                reason: "workflow must define at least one node".to_owned(),
            });
        }

        // 1. Unique node IDs
        let mut seen_node_ids = std::collections::HashSet::new();
        for node in &self.nodes {
            if !seen_node_ids.insert(&node.id) {
                return Err(WorkflowError::Validation {
                    reason: format!("duplicate node id '{}'", node.id),
                });
            }
        }

        // 2. Role resolution: each node.role_id must resolve to a known role definition
        for node in &self.nodes {
            let role_known = self.roles.iter().any(|r| r.id == node.role_id)
                || node.role_id.0 == "implementer"
                || node.role_id.0 == "verifier";
            if !role_known {
                return Err(WorkflowError::Validation {
                    reason: format!(
                        "unknown role reference '{}' in node '{}'",
                        node.role_id, node.id
                    ),
                });
            }
        }

        // 3. Transition endpoints exist
        for t in &self.transitions {
            if !self.nodes.iter().any(|n| n.id == t.from_node) {
                return Err(WorkflowError::Validation {
                    reason: format!("transition from unknown node '{}'", t.from_node),
                });
            }
            if !self.nodes.iter().any(|n| n.id == t.to_node) {
                return Err(WorkflowError::Validation {
                    reason: format!("transition to unknown node '{}'", t.to_node),
                });
            }
        }

        // 4. Reachability: every node must be reachable from the start node (nodes[0])
        let start_node = &self.nodes[0].id;
        let mut reachable = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        reachable.insert(start_node.clone());
        queue.push_back(start_node.clone());

        while let Some(current) = queue.pop_front() {
            for t in &self.transitions {
                if t.from_node == current && reachable.insert(t.to_node.clone()) {
                    queue.push_back(t.to_node.clone());
                }
            }
        }

        for node in &self.nodes {
            if !reachable.contains(&node.id) {
                return Err(WorkflowError::Validation {
                    reason: format!(
                        "unreachable node '{}' from start node '{}'",
                        node.id, start_node
                    ),
                });
            }
        }

        // 5. Typed I/O compatibility:
        // Transition required_artifact_types must be produced by from_node.
        // Node consumed_artifact_types must be produced by from_node or transition.
        for t in &self.transitions {
            let from_node = self.nodes.iter().find(|n| n.id == t.from_node).unwrap();
            let to_node = self.nodes.iter().find(|n| n.id == t.to_node).unwrap();

            for req in &t.required_artifact_types {
                if !from_node.produced_artifact_types.contains(req) {
                    return Err(WorkflowError::Validation {
                        reason: format!(
                            "type mismatch: transition from '{}' to '{}' requires artifact '{}' which is not produced by '{}'",
                            t.from_node, t.to_node, req, t.from_node
                        ),
                    });
                }
            }

            for consumed in &to_node.consumed_artifact_types {
                if !from_node.produced_artifact_types.contains(consumed)
                    && !t.required_artifact_types.contains(consumed)
                {
                    return Err(WorkflowError::Validation {
                        reason: format!(
                            "type mismatch: node '{}' consumes artifact '{}' which is not produced by predecessor '{}'",
                            t.to_node, consumed, t.from_node
                        ),
                    });
                }
            }
        }

        // 6. Delegation edges: if transition crosses roles, role_a must allow delegation to role_b
        for t in &self.transitions {
            let from_node = self.nodes.iter().find(|n| n.id == t.from_node).unwrap();
            let to_node = self.nodes.iter().find(|n| n.id == t.to_node).unwrap();

            if from_node.role_id != to_node.role_id {
                let from_role_def = self.roles.iter().find(|r| r.id == from_node.role_id);
                if let Some(role) = from_role_def {
                    if !role.allowed_delegations.contains(&to_node.role_id) {
                        return Err(WorkflowError::Validation {
                            reason: format!(
                                "undeclared delegation edge: role '{}' in node '{}' cannot delegate to role '{}' in node '{}'",
                                from_node.role_id, t.from_node, to_node.role_id, t.to_node
                            ),
                        });
                    }
                }
            }
        }

        // 7. Cycle detection and bound validation
        self.validate_cycle_bounds()?;

        Ok(())
    }

    fn validate_cycle_bounds(&self) -> Result<(), WorkflowError> {
        // Reject unconditional self-loops
        for t in &self.transitions {
            if t.from_node == t.to_node && matches!(t.condition, TransitionCondition::Always) {
                return Err(WorkflowError::Validation {
                    reason: format!(
                        "unbounded cycle detected: unconditional self-loop on node '{}'",
                        t.from_node
                    ),
                });
            }
        }

        // Check each node for cycles
        let mut states: std::collections::HashMap<&str, DfsState> = self
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), DfsState::Unvisited))
            .collect();

        for node in &self.nodes {
            if states[node.id.as_str()] == DfsState::Unvisited {
                let mut path = Vec::new();
                self.dfs_cycle_check(node.id.as_str(), &mut states, &mut path)?;
            }
        }

        Ok(())
    }

    fn dfs_cycle_check<'a>(
        &'a self,
        current: &'a str,
        states: &mut std::collections::HashMap<&'a str, DfsState>,
        path: &mut Vec<&'a str>,
    ) -> Result<(), WorkflowError> {
        states.insert(current, DfsState::Visiting);
        path.push(current);

        for t in &self.transitions {
            if t.from_node == current && t.from_node != t.to_node {
                let target = t.to_node.as_str();
                match states.get(target) {
                    Some(DfsState::Visiting) => {
                        let cycle_start_idx = path.iter().position(|&x| x == target).unwrap_or(0);
                        let cycle_nodes = &path[cycle_start_idx..];

                        // All transitions along the cycle must not be unconditional Always
                        for i in 0..cycle_nodes.len() {
                            let u = cycle_nodes[i];
                            let v = if i + 1 < cycle_nodes.len() {
                                cycle_nodes[i + 1]
                            } else {
                                target
                            };
                            let trans = self
                                .transitions
                                .iter()
                                .find(|tr| tr.from_node == u && tr.to_node == v);
                            if let Some(tr) = trans {
                                if matches!(tr.condition, TransitionCondition::Always) {
                                    return Err(WorkflowError::Validation {
                                        reason: format!(
                                            "unbounded cycle detected: unconditional transition from '{}' to '{}' in cycle",
                                            u, v
                                        ),
                                    });
                                }
                            }
                        }

                        // Cycle requires finite execution bounds
                        if self.default_budget.max_role_activations == 0
                            || self.default_budget.max_attempts_per_role == 0
                        {
                            return Err(WorkflowError::Validation {
                                reason:
                                    "unbounded cycle: budget has zero activation or attempt limit"
                                        .to_owned(),
                            });
                        }
                    }
                    Some(DfsState::Unvisited) => {
                        self.dfs_cycle_check(target, states, path)?;
                    }
                    _ => {}
                }
            }
        }

        path.pop();
        states.insert(current, DfsState::Visited);
        Ok(())
    }

    /// Creates and validates a new `WorkflowRun` under this workflow definition.
    pub fn create_run(
        &self,
        task_id: TaskId,
        execution_grant_id: ExecutionGrantId,
        config_snapshot_id: ConfigSnapshotId,
    ) -> Result<WorkflowRun, WorkflowError> {
        self.validate()?;
        Ok(WorkflowRun::new(
            task_id,
            execution_grant_id,
            self.id.clone(),
            config_snapshot_id,
        ))
    }
}

// ---------------------------------------------------------------------------
// 9. WorkflowRun
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunState {
    Pending,
    Running,
    Suspended,
    WaitingForAttention,
    Completed,
    Failed,
    Cancelled,
}

impl WorkflowRunState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: WorkflowRunId,
    pub task_id: TaskId,
    pub execution_grant_id: ExecutionGrantId,
    pub workflow_definition_id: WorkflowDefinitionId,
    pub state: WorkflowRunState,
    pub config_snapshot_id: ConfigSnapshotId,
    pub started_at: String,
    pub completed_at: Option<String>,
}

impl WorkflowRun {
    pub fn new(
        task_id: TaskId,
        execution_grant_id: ExecutionGrantId,
        workflow_definition_id: WorkflowDefinitionId,
        config_snapshot_id: ConfigSnapshotId,
    ) -> Self {
        Self {
            id: WorkflowRunId::generate(),
            task_id,
            execution_grant_id,
            workflow_definition_id,
            state: WorkflowRunState::Pending,
            config_snapshot_id,
            started_at: now_timestamp(),
            completed_at: None,
        }
    }

    pub fn start(&mut self) -> Result<(), WorkflowError> {
        if self.state != WorkflowRunState::Pending {
            return Err(WorkflowError::RunError(format!(
                "cannot start run from state {:?}",
                self.state
            )));
        }
        self.state = WorkflowRunState::Running;
        Ok(())
    }

    pub fn complete(&mut self) {
        self.state = WorkflowRunState::Completed;
        self.completed_at = Some(now_timestamp());
    }

    pub fn fail(&mut self) {
        self.state = WorkflowRunState::Failed;
        self.completed_at = Some(now_timestamp());
    }

    pub fn cancel(&mut self) {
        self.state = WorkflowRunState::Cancelled;
        self.completed_at = Some(now_timestamp());
    }
}

// ---------------------------------------------------------------------------
// 11. RoleDefinition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleWriteMode {
    ReadOnly,
    IsolatedWorktree,
    CandidateProducer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDefinition {
    pub id: RoleDefinitionId,
    pub purpose: String,
    pub write_mode: RoleWriteMode,
    pub allowed_delegations: Vec<RoleDefinitionId>,
}

// ---------------------------------------------------------------------------
// 12. RoleInstance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleInstanceState {
    Created,
    WaitingInputs,
    Ready,
    Active,
    WaitingChildren,
    ResultPending,
    Completed,
    Blocked,
    Failed,
    Cancelled,
}

impl RoleInstanceState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleInstance {
    pub id: RoleInstanceId,
    pub workflow_run_id: WorkflowRunId,
    pub role_definition_id: RoleDefinitionId,
    pub generation: u32,
    pub state: RoleInstanceState,
    pub parent_role_instance_id: Option<RoleInstanceId>,
    pub accepted_result_artifact: Option<String>,
    pub attempts_count: u32,
}

impl RoleInstance {
    pub fn new(
        workflow_run_id: WorkflowRunId,
        role_definition_id: RoleDefinitionId,
        generation: u32,
        parent: Option<RoleInstanceId>,
    ) -> Self {
        Self {
            id: RoleInstanceId::generate(&role_definition_id),
            workflow_run_id,
            role_definition_id,
            generation,
            state: RoleInstanceState::Created,
            parent_role_instance_id: parent,
            accepted_result_artifact: None,
            attempts_count: 0,
        }
    }

    /// Spawns a new attempt for this role instance.
    ///
    /// Retrying an attempt does NOT create a new semantic role instance (invariant).
    pub fn spawn_attempt(
        &mut self,
        target: ExecutionTarget,
        budget: &ExecutionBudget,
    ) -> Result<RoleAttempt, WorkflowError> {
        if self.attempts_count >= budget.max_attempts_per_role {
            return Err(WorkflowError::BudgetExhausted {
                reason: format!(
                    "exceeded maximum attempts per role ({})",
                    budget.max_attempts_per_role
                ),
            });
        }

        self.attempts_count += 1;
        self.state = RoleInstanceState::Active;

        Ok(RoleAttempt {
            id: RoleAttemptId::generate(&self.id, self.attempts_count),
            role_instance_id: self.id.clone(),
            attempt_no: self.attempts_count,
            execution_target: target,
            state: RoleAttemptState::Queued,
            lease_id: None,
            fencing_token: None,
            runtime_outcome: None,
            result_artifact_id: None,
            started_at: Some(now_timestamp()),
            ended_at: None,
            runtime_handle_id: None,
            runtime_backplane_id: None,
        })
    }

    /// Accepts an artifact as the required result of this role instance, completing it.
    pub fn accept_result(&mut self, artifact_id: String) {
        self.state = RoleInstanceState::Completed;
        self.accepted_result_artifact = Some(artifact_id);
    }

    /// Marks the role instance as failed when attempts are exhausted or execution is blocked.
    pub fn fail(&mut self) {
        self.state = RoleInstanceState::Failed;
    }

    /// Marks the role instance as cancelled.
    pub fn cancel(&mut self) {
        self.state = RoleInstanceState::Cancelled;
    }
}

// ---------------------------------------------------------------------------
// 13 & 14. RoleAttempt and ExecutionTarget
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionTarget {
    pub provider_id: String,
    pub runtime_profile_id: String,
    pub provider_adapter_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleAttemptState {
    Queued,
    Provisioning,
    Starting,
    Running,
    ResultPending,
    Completed,
    Interrupted,
    Failed,
    Cancelled,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleAttempt {
    pub id: RoleAttemptId,
    pub role_instance_id: RoleInstanceId,
    pub attempt_no: u32,
    pub execution_target: ExecutionTarget,
    pub state: RoleAttemptState,
    pub lease_id: Option<String>,
    pub fencing_token: Option<u64>,
    pub runtime_outcome: Option<String>,
    pub result_artifact_id: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// Backplane handle this attempt was launched as, once known. Durable so a
    /// restarted daemon can reconcile the process it really launched.
    pub runtime_handle_id: Option<String>,
    /// Backplane that owns `runtime_handle_id`.
    pub runtime_backplane_id: Option<String>,
}

impl RoleAttempt {
    pub fn finish(&mut self, outcome: String, artifact_id: Option<String>) {
        self.state = RoleAttemptState::Completed;
        self.runtime_outcome = Some(outcome);
        self.result_artifact_id = artifact_id;
        self.ended_at = Some(now_timestamp());
    }

    pub fn fail(&mut self, reason: String) {
        self.state = RoleAttemptState::Failed;
        self.runtime_outcome = Some(reason);
        self.ended_at = Some(now_timestamp());
    }
}

// ---------------------------------------------------------------------------
// 23, 24, 25. Clarification, Decision, ScopeExpansion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationRequest {
    pub task_id: TaskId,
    pub question: String,
    pub required_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub task_id: TaskId,
    pub kind: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeExpansionRequest {
    pub task_id: TaskId,
    pub current_generation: u32,
    pub requested_objective: String,
    pub reason: String,
    pub affected_constraints: Vec<String>,
    pub additional_work_estimate: Option<String>,
}

// ---------------------------------------------------------------------------
// 22. ApprovalRequest & Grant
//
// The Phase 03 placeholder shell previously here (bare String digests, no
// expiration/replay protection) is superseded by the Phase 13 exact
// approval contract in `crate::privileged` (`ApprovalRequest`,
// `ApprovalGrant`, SHA-256 `ParameterDigest`), which is provider-agnostic
// and used for privileged Git operations.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Configuration Snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub id: ConfigSnapshotId,
    pub workflow_version: u32,
    pub budget: ExecutionBudget,
    pub policy_digest: String,
    pub captured_at: String,
}

impl ConfigSnapshot {
    pub fn capture(workflow_version: u32, budget: ExecutionBudget, policy_digest: String) -> Self {
        Self {
            id: ConfigSnapshotId::generate(),
            workflow_version,
            budget,
            policy_digest,
            captured_at: now_timestamp(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_immutability_and_generation_progression() {
        let task_id = TaskId::generate();
        let intent_g1 = ApprovedIntent::new(
            task_id,
            1,
            "Build Phase 03".to_owned(),
            vec!["criteria 1".to_owned()],
            vec!["no regression".to_owned()],
            vec![],
            "operator".to_owned(),
        );

        assert_eq!(intent_g1.generation, 1);
        assert_eq!(intent_g1.objective, "Build Phase 03");

        // Scope change produces generation 2; generation 1 remains immutable
        let intent_g2 = intent_g1.create_next_generation(
            "Build Phase 03 and verify in memory".to_owned(),
            vec!["criteria 1".to_owned(), "criteria 2".to_owned()],
            vec!["no regression".to_owned()],
            "operator".to_owned(),
        );

        assert_eq!(intent_g2.generation, 2);
        assert_eq!(intent_g2.objective, "Build Phase 03 and verify in memory");
        assert_eq!(intent_g1.generation, 1);
        assert_eq!(intent_g1.objective, "Build Phase 03");
        assert_ne!(intent_g1.id, intent_g2.id);
    }

    #[test]
    fn attempt_retry_does_not_create_new_semantic_role_instance() {
        let run_id = WorkflowRunId::generate();
        let role_def = RoleDefinitionId::new("implementer");
        let mut role = RoleInstance::new(run_id, role_def, 1, None);

        let target = ExecutionTarget {
            provider_id: "codex".to_owned(),
            runtime_profile_id: "codex/default".to_owned(),
            provider_adapter_version: "0.1.0".to_owned(),
        };
        let budget = ExecutionBudget::default();

        // First attempt
        let mut att1 = role.spawn_attempt(target.clone(), &budget).unwrap();
        assert_eq!(att1.attempt_no, 1);
        assert_eq!(role.attempts_count, 1);
        att1.fail("compiler error".to_owned());

        // Second attempt on the same role instance
        let att2 = role.spawn_attempt(target, &budget).unwrap();
        assert_eq!(att2.attempt_no, 2);
        assert_eq!(role.attempts_count, 2);

        // Crucial invariant: RoleInstance identity did not change!
        assert_eq!(att1.role_instance_id, role.id);
        assert_eq!(att2.role_instance_id, role.id);
        assert_ne!(att1.id, att2.id);
    }

    #[test]
    fn workflow_definition_validation_rejects_missing_node() {
        let wf = WorkflowDefinition {
            id: WorkflowDefinitionId::new("default"),
            version: 1,
            roles: vec![],
            nodes: vec![WorkflowNodeDefinition {
                id: "implement".to_owned(),
                role_id: RoleDefinitionId::new("implementer"),
                consumed_artifact_types: vec![],
                produced_artifact_types: vec![],
            }],
            transitions: vec![WorkflowTransitionDefinition {
                from_node: "implement".to_owned(),
                to_node: "missing_node".to_owned(),
                condition: TransitionCondition::RoleAttemptSucceeded,
                required_artifact_types: vec![],
            }],
            default_budget: ExecutionBudget::default(),
            content_hash: "hash".to_owned(),
        };

        assert!(wf.validate().is_err());
    }

    #[test]
    fn workflow_definition_validation_rejects_unknown_role() {
        let wf = WorkflowDefinition {
            id: WorkflowDefinitionId::new("unknown-role"),
            version: 1,
            roles: vec![],
            nodes: vec![WorkflowNodeDefinition {
                id: "step1".to_owned(),
                role_id: RoleDefinitionId::new("unknown-arbitrary-role"),
                consumed_artifact_types: vec![],
                produced_artifact_types: vec![],
            }],
            transitions: vec![],
            default_budget: ExecutionBudget::default(),
            content_hash: "hash".to_owned(),
        };

        match wf.validate() {
            Err(WorkflowError::Validation { reason }) => {
                assert!(reason.contains("unknown role reference"));
            }
            other => panic!("expected unknown role validation error, got {other:?}"),
        }
    }

    #[test]
    fn workflow_definition_validation_rejects_unreachable_node() {
        let wf = WorkflowDefinition {
            id: WorkflowDefinitionId::new("unreachable"),
            version: 1,
            roles: vec![],
            nodes: vec![
                WorkflowNodeDefinition {
                    id: "node-a".to_owned(),
                    role_id: RoleDefinitionId::new("implementer"),
                    consumed_artifact_types: vec![],
                    produced_artifact_types: vec![],
                },
                WorkflowNodeDefinition {
                    id: "node-b".to_owned(),
                    role_id: RoleDefinitionId::new("verifier"),
                    consumed_artifact_types: vec![],
                    produced_artifact_types: vec![],
                },
                WorkflowNodeDefinition {
                    id: "orphan".to_owned(),
                    role_id: RoleDefinitionId::new("implementer"),
                    consumed_artifact_types: vec![],
                    produced_artifact_types: vec![],
                },
            ],
            transitions: vec![WorkflowTransitionDefinition {
                from_node: "node-a".to_owned(),
                to_node: "node-b".to_owned(),
                condition: TransitionCondition::RoleAttemptSucceeded,
                required_artifact_types: vec![],
            }],
            default_budget: ExecutionBudget::default(),
            content_hash: "hash".to_owned(),
        };

        match wf.validate() {
            Err(WorkflowError::Validation { reason }) => {
                assert!(reason.contains("unreachable node 'orphan'"));
            }
            other => panic!("expected unreachable node error, got {other:?}"),
        }
    }

    #[test]
    fn workflow_definition_validation_rejects_artifact_type_mismatch() {
        let wf = WorkflowDefinition {
            id: WorkflowDefinitionId::new("mismatch"),
            version: 1,
            roles: vec![],
            nodes: vec![
                WorkflowNodeDefinition {
                    id: "node-a".to_owned(),
                    role_id: RoleDefinitionId::new("implementer"),
                    consumed_artifact_types: vec![],
                    produced_artifact_types: vec!["code_diff".to_owned()],
                },
                WorkflowNodeDefinition {
                    id: "node-b".to_owned(),
                    role_id: RoleDefinitionId::new("verifier"),
                    consumed_artifact_types: vec!["binary_release".to_owned()],
                    produced_artifact_types: vec![],
                },
            ],
            transitions: vec![WorkflowTransitionDefinition {
                from_node: "node-a".to_owned(),
                to_node: "node-b".to_owned(),
                condition: TransitionCondition::RoleAttemptSucceeded,
                required_artifact_types: vec!["binary_release".to_owned()],
            }],
            default_budget: ExecutionBudget::default(),
            content_hash: "hash".to_owned(),
        };

        match wf.validate() {
            Err(WorkflowError::Validation { reason }) => {
                assert!(reason.contains("type mismatch"));
            }
            other => panic!("expected type mismatch error, got {other:?}"),
        }
    }

    #[test]
    fn workflow_definition_validation_rejects_undeclared_delegation() {
        let role_a = RoleDefinition {
            id: RoleDefinitionId::new("role-a"),
            purpose: "Purpose A".to_owned(),
            write_mode: RoleWriteMode::ReadOnly,
            allowed_delegations: vec![], // No delegation allowed!
        };
        let role_b = RoleDefinition {
            id: RoleDefinitionId::new("role-b"),
            purpose: "Purpose B".to_owned(),
            write_mode: RoleWriteMode::ReadOnly,
            allowed_delegations: vec![],
        };

        let wf = WorkflowDefinition {
            id: WorkflowDefinitionId::new("undeclared-delegation"),
            version: 1,
            roles: vec![role_a, role_b],
            nodes: vec![
                WorkflowNodeDefinition {
                    id: "node-a".to_owned(),
                    role_id: RoleDefinitionId::new("role-a"),
                    consumed_artifact_types: vec![],
                    produced_artifact_types: vec![],
                },
                WorkflowNodeDefinition {
                    id: "node-b".to_owned(),
                    role_id: RoleDefinitionId::new("role-b"),
                    consumed_artifact_types: vec![],
                    produced_artifact_types: vec![],
                },
            ],
            transitions: vec![WorkflowTransitionDefinition {
                from_node: "node-a".to_owned(),
                to_node: "node-b".to_owned(),
                condition: TransitionCondition::RoleAttemptSucceeded,
                required_artifact_types: vec![],
            }],
            default_budget: ExecutionBudget::default(),
            content_hash: "hash".to_owned(),
        };

        match wf.validate() {
            Err(WorkflowError::Validation { reason }) => {
                assert!(reason.contains("undeclared delegation edge"));
            }
            other => panic!("expected undeclared delegation error, got {other:?}"),
        }
    }

    #[test]
    fn workflow_definition_validation_rejects_unbounded_cycles() {
        // Unconditional Always self-loop
        let wf_self = WorkflowDefinition {
            id: WorkflowDefinitionId::new("self-loop"),
            version: 1,
            roles: vec![],
            nodes: vec![WorkflowNodeDefinition {
                id: "loop-node".to_owned(),
                role_id: RoleDefinitionId::new("implementer"),
                consumed_artifact_types: vec![],
                produced_artifact_types: vec![],
            }],
            transitions: vec![WorkflowTransitionDefinition {
                from_node: "loop-node".to_owned(),
                to_node: "loop-node".to_owned(),
                condition: TransitionCondition::Always,
                required_artifact_types: vec![],
            }],
            default_budget: ExecutionBudget::default(),
            content_hash: "hash".to_owned(),
        };

        assert!(wf_self.validate().is_err());

        // Unconditional cycle A -> B -> A
        let wf_cycle = WorkflowDefinition {
            id: WorkflowDefinitionId::new("unconditional-cycle"),
            version: 1,
            roles: vec![],
            nodes: vec![
                WorkflowNodeDefinition {
                    id: "node-a".to_owned(),
                    role_id: RoleDefinitionId::new("implementer"),
                    consumed_artifact_types: vec![],
                    produced_artifact_types: vec![],
                },
                WorkflowNodeDefinition {
                    id: "node-b".to_owned(),
                    role_id: RoleDefinitionId::new("verifier"),
                    consumed_artifact_types: vec![],
                    produced_artifact_types: vec![],
                },
            ],
            transitions: vec![
                WorkflowTransitionDefinition {
                    from_node: "node-a".to_owned(),
                    to_node: "node-b".to_owned(),
                    condition: TransitionCondition::Always,
                    required_artifact_types: vec![],
                },
                WorkflowTransitionDefinition {
                    from_node: "node-b".to_owned(),
                    to_node: "node-a".to_owned(),
                    condition: TransitionCondition::Always,
                    required_artifact_types: vec![],
                },
            ],
            default_budget: ExecutionBudget::default(),
            content_hash: "hash".to_owned(),
        };

        assert!(wf_cycle.validate().is_err());
    }
}
